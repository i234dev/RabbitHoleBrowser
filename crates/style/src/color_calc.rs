//! Typed math expressions (CSS Values 4 §10) as color components take them:
//! numbers, percentages, angles and lengths, with the channel keywords of
//! relative colors (`r`, `h`, `alpha`, ...) as unresolved `<number>`s.
//! Expressions are kept as trees so specified values serialize in their
//! simplified form and computed values evaluate them.

use crate::values::components;
use rabbithole_css::Token;

/// The dimensions of a value: exponents of angle, length and percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ty {
    pub angle: i8,
    pub length: i8,
    pub percent: i8,
}

impl Ty {
    pub const NUMBER: Ty = Ty { angle: 0, length: 0, percent: 0 };
    pub const ANGLE: Ty = Ty { angle: 1, length: 0, percent: 0 };
    pub const PERCENT: Ty = Ty { angle: 0, length: 0, percent: 1 };
    pub const LENGTH: Ty = Ty { angle: 0, length: 1, percent: 0 };
    fn mul(self, o: Ty) -> Ty {
        Ty { angle: self.angle + o.angle, length: self.length + o.length, percent: self.percent + o.percent }
    }
    fn inv(self) -> Ty {
        Ty { angle: -self.angle, length: -self.length, percent: -self.percent }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Calc {
    Num(f64),
    Pct(f64),
    /// A dimension with its (lower-case) unit.
    Dim(f64, String),
    /// A channel keyword.
    Key(String),
    Sum(Vec<Calc>),
    Prod(Vec<Calc>),
    Neg(Box<Calc>),
    Inv(Box<Calc>),
    /// A math function; `round` carries its strategy in the name
    /// (`round`, `round-up`, `round-down`, `round-to-zero`).
    Func(String, Vec<Calc>),
}

const MATH_FUNCTIONS: [&str; 22] = [
    "calc", "min", "max", "clamp", "round", "mod", "rem", "abs", "sign", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "pow",
    "sqrt", "hypot", "log", "exp", "-webkit-calc",
];

pub fn is_math_function(name: &str) -> bool {
    MATH_FUNCTIONS.iter().any(|f| name.eq_ignore_ascii_case(f))
}

fn angle_deg(v: f64, unit: &str) -> Option<f64> {
    Some(match unit {
        "deg" => v,
        "rad" => v.to_degrees(),
        "grad" => v * 0.9,
        "turn" => v * 360.0,
        _ => return None,
    })
}

/// Absolute lengths in px; relative units only resolve with a context.
fn absolute_px(v: f64, unit: &str) -> Option<f64> {
    Some(match unit {
        "px" => v,
        "in" => v * 96.0,
        "cm" => v * 96.0 / 2.54,
        "mm" => v * 96.0 / 25.4,
        "q" => v * 96.0 / 101.6,
        "pt" => v * 4.0 / 3.0,
        "pc" => v * 16.0,
        _ => return None,
    })
}

fn is_length_unit(unit: &str) -> bool {
    absolute_px(1.0, unit).is_some()
        || matches!(unit, "em" | "rem" | "ex" | "rex" | "ch" | "rch" | "ic" | "ric" | "lh" | "rlh" | "cap" | "rcap")
        || ["vw", "vh", "vi", "vb", "vmin", "vmax"].iter().any(|u| unit == *u || unit.strip_prefix(['s', 'l', 'd']) == Some(u))
        || matches!(unit, "cqw" | "cqh" | "cqi" | "cqb" | "cqmin" | "cqmax")
}

fn unit_ty(unit: &str) -> Option<Ty> {
    if angle_deg(1.0, unit).is_some() {
        Some(Ty::ANGLE)
    } else if is_length_unit(unit) {
        Some(Ty::LENGTH)
    } else {
        None
    }
}

/// Parse a math function or a numeric token. `keywords` are the names
/// usable as `<number>`s (channel keywords).
pub fn parse(c: &[Token], keywords: &[&str]) -> Option<Calc> {
    let node = parse_value(c, keywords, true)?;
    node.ty()?;
    Some(node)
}

/// A math function that is a `<length-percentage>`.
pub fn parse_length_percentage(c: &[Token]) -> Option<Calc> {
    let node = parse_value(c, &[], true)?;
    (node.ty_with(Ty::LENGTH)? == Ty::LENGTH).then_some(node)
}

fn ident_constant(k: &str) -> Option<f64> {
    Some(match k.to_ascii_lowercase().as_str() {
        "e" => std::f64::consts::E,
        "pi" => std::f64::consts::PI,
        "infinity" => f64::INFINITY,
        "-infinity" => f64::NEG_INFINITY,
        "nan" => f64::NAN,
        _ => return None,
    })
}

fn parse_value(c: &[Token], keywords: &[&str], top: bool) -> Option<Calc> {
    match c {
        [Token::Number(n)] if !top => Some(Calc::Num(f64::from(*n))),
        [Token::Percentage(n)] if !top => Some(Calc::Pct(f64::from(*n))),
        [Token::Dimension(n, u)] if !top => {
            let u = u.to_ascii_lowercase();
            unit_ty(&u)?;
            Some(Calc::Dim(f64::from(*n), u))
        }
        [Token::Ident(k)] if !top => {
            let lower = k.to_ascii_lowercase();
            if keywords.contains(&lower.as_str()) {
                Some(Calc::Key(lower))
            } else {
                ident_constant(k).map(Calc::Num)
            }
        }
        [Token::OpenParen, ..] if !top => {
            let inner = match c {
                [_, inner @ .., Token::CloseParen] => inner,
                [_, inner @ ..] => inner,
                _ => return None,
            };
            parse_sum(inner, keywords)
        }
        [Token::Function(f), ..] => {
            let name = f.to_ascii_lowercase();
            if !is_math_function(&name) {
                return None;
            }
            let inner = match c {
                [_, inner @ .., Token::CloseParen] => inner,
                [_, inner @ ..] => inner,
                _ => return None,
            };
            let args: Vec<&[Token]> = split_args(inner);
            let arg = |i: usize| -> Option<Calc> { parse_sum(args.get(i)?, keywords) };
            let all = || -> Option<Vec<Calc>> { args.iter().map(|a| parse_sum(a, keywords)).collect() };
            let node = match name.as_str() {
                "calc" | "-webkit-calc" if args.len() == 1 => arg(0)?,
                "min" | "max" | "hypot" if !args.is_empty() => Calc::Func(name, all()?),
                "clamp" if args.len() == 3 => {
                    let mut v = Vec::new();
                    for a in &args {
                        if matches!(components(a).as_slice(), [k] if crate::values::is_ident(k, "none")) {
                            v.push(Calc::Key("none".into()));
                        } else {
                            v.push(parse_sum(a, keywords)?);
                        }
                    }
                    Calc::Func(name, v)
                }
                "round" if (1..=3).contains(&args.len()) => {
                    let first: Vec<&[Token]> = components(args[0]);
                    let strategy = match first.as_slice() {
                        [k] => crate::values::ident(k).filter(|k| matches!(k.as_str(), "nearest" | "up" | "down" | "to-zero")),
                        _ => None,
                    };
                    let rest: Vec<&[Token]> = if strategy.is_some() { args[1..].to_vec() } else { args.clone() };
                    if rest.is_empty() || rest.len() > 2 {
                        return None;
                    }
                    let name = match strategy.as_deref() {
                        Some("up") => "round-up",
                        Some("down") => "round-down",
                        Some("to-zero") => "round-to-zero",
                        _ => "round",
                    };
                    let v: Option<Vec<Calc>> = rest.iter().map(|a| parse_sum(a, keywords)).collect();
                    Calc::Func(name.into(), v?)
                }
                "mod" | "rem" | "atan2" | "pow" if args.len() == 2 => Calc::Func(name, all()?),
                "log" if (1..=2).contains(&args.len()) => Calc::Func(name, all()?),
                "abs" | "sign" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sqrt" | "exp" if args.len() == 1 => Calc::Func(name, all()?),
                _ => return None,
            };
            Some(node)
        }
        _ => None,
    }
}

fn split_args(toks: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Token::Function(_) | Token::OpenParen => depth += 1,
            Token::CloseParen => depth -= 1,
            Token::Comma if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&toks[start..]);
    out
}

fn parse_sum(toks: &[Token], keywords: &[&str]) -> Option<Calc> {
    // `+` and `-` must be surrounded by whitespace.
    let comps: Vec<&[Token]> = components(toks);
    if comps.is_empty() {
        return None;
    }
    // Check whitespace around + and - in the raw token stream.
    for (i, t) in toks.iter().enumerate() {
        if matches!(t, Token::Delim('+') | Token::Delim('-')) {
            let before = i > 0 && matches!(toks[i - 1], Token::Whitespace);
            let after = matches!(toks.get(i + 1), Some(Token::Whitespace));
            if !before || !after {
                // Only top-level delimiters matter; nested ones are inside
                // function/paren components and never reach this level.
                let depth = toks[..i].iter().fold(0i32, |d, t| match t {
                    Token::Function(_) | Token::OpenParen => d + 1,
                    Token::CloseParen => d - 1,
                    _ => d,
                });
                if depth == 0 {
                    return None;
                }
            }
        }
    }
    let mut terms: Vec<Calc> = Vec::new();
    let mut current: Vec<&[Token]> = Vec::new();
    let mut pending_neg = false;
    for c in comps {
        match c {
            [Token::Delim('+')] | [Token::Delim('-')] => {
                if current.is_empty() {
                    return None;
                }
                let t = parse_product(&current, keywords)?;
                terms.push(if pending_neg { Calc::Neg(Box::new(t)) } else { t });
                current.clear();
                pending_neg = matches!(c, [Token::Delim('-')]);
            }
            _ => current.push(c),
        }
    }
    if current.is_empty() {
        return None;
    }
    let t = parse_product(&current, keywords)?;
    terms.push(if pending_neg { Calc::Neg(Box::new(t)) } else { t });
    Some(if terms.len() == 1 { terms.pop()? } else { Calc::Sum(terms) })
}

fn parse_product(comps: &[&[Token]], keywords: &[&str]) -> Option<Calc> {
    let mut factors = Vec::new();
    let mut invert = false;
    let mut expect_value = true;
    for c in comps {
        match c {
            [Token::Delim('*')] | [Token::Delim('/')] if !expect_value => {
                invert = matches!(c, [Token::Delim('/')]);
                expect_value = true;
            }
            _ if expect_value => {
                let v = parse_value(c, keywords, false)?;
                factors.push(if invert { Calc::Inv(Box::new(v)) } else { v });
                expect_value = false;
            }
            _ => return None,
        }
    }
    if expect_value {
        return None;
    }
    Some(if factors.len() == 1 { factors.pop()? } else { Calc::Prod(factors) })
}

impl Calc {
    /// The type of the expression, `None` when it is invalid.
    pub fn ty(&self) -> Option<Ty> {
        self.ty_with(Ty::PERCENT)
    }

    /// The type with percentages typed as `pct` (lengths where they
    /// resolve against one).
    pub fn ty_with(&self, pct: Ty) -> Option<Ty> {
        match self {
            Calc::Num(_) | Calc::Key(_) => Some(Ty::NUMBER),
            Calc::Pct(_) => Some(pct),
            Calc::Dim(_, u) => unit_ty(u),
            Calc::Sum(v) => {
                let t = v.first()?.ty_with(pct)?;
                for c in &v[1..] {
                    if c.ty_with(pct)? != t {
                        return None;
                    }
                }
                Some(t)
            }
            Calc::Prod(v) => v.iter().try_fold(Ty::NUMBER, |acc, c| Some(acc.mul(c.ty_with(pct)?))),
            Calc::Neg(c) => c.ty_with(pct),
            Calc::Inv(c) => c.ty_with(pct).map(Ty::inv),
            Calc::Func(name, args) => {
                let same = || -> Option<Ty> {
                    let mut t = None;
                    for a in args {
                        if matches!(a, Calc::Key(k) if k == "none") {
                            continue;
                        }
                        let at = a.ty_with(pct)?;
                        if t.is_some_and(|t| t != at) {
                            return None;
                        }
                        t = Some(at);
                    }
                    t
                };
                match name.as_str() {
                    "min" | "max" | "clamp" | "hypot" | "mod" | "rem" | "abs" => same(),
                    n if n.starts_with("round") => same(),
                    "sign" => {
                        args[0].ty_with(pct)?;
                        Some(Ty::NUMBER)
                    }
                    "sin" | "cos" | "tan" => {
                        let t = args[0].ty_with(pct)?;
                        (t == Ty::NUMBER || t == Ty::ANGLE).then_some(Ty::NUMBER)
                    }
                    "asin" | "acos" | "atan" => (args[0].ty_with(pct)? == Ty::NUMBER).then_some(Ty::ANGLE),
                    "atan2" => same().map(|_| Ty::ANGLE),
                    "pow" | "sqrt" | "log" | "exp" => args.iter().all(|a| a.ty_with(pct) == Some(Ty::NUMBER)).then_some(Ty::NUMBER),
                    _ => None,
                }
            }
        }
    }

    /// Whether the value has lengths in units only known at computed-value
    /// time (font- or viewport-relative).
    pub fn has_relative_units(&self) -> bool {
        match self {
            Calc::Dim(_, u) => angle_deg(1.0, u).is_none() && absolute_px(1.0, u).is_none(),
            Calc::Sum(v) | Calc::Prod(v) | Calc::Func(_, v) => v.iter().any(Calc::has_relative_units),
            Calc::Neg(c) | Calc::Inv(c) => c.has_relative_units(),
            _ => false,
        }
    }

    /// Whether the value depends on channel keywords.
    pub fn has_keywords(&self) -> bool {
        match self {
            Calc::Key(k) => k != "none",
            Calc::Sum(v) | Calc::Prod(v) | Calc::Func(_, v) => v.iter().any(Calc::has_keywords),
            Calc::Neg(c) | Calc::Inv(c) => c.has_keywords(),
            _ => false,
        }
    }

    /// Evaluate to (value, type): angles in degrees, lengths in px,
    /// percentages as percentages. `key` resolves channel keywords and
    /// `length` relative length units.
    pub fn eval(&self, key: &dyn Fn(&str) -> Option<f64>, length: &dyn Fn(f64, &str) -> Option<f64>) -> Option<f64> {
        Some(match self {
            Calc::Num(n) => *n,
            Calc::Pct(n) => *n,
            Calc::Dim(n, u) => angle_deg(*n, u).or_else(|| absolute_px(*n, u)).or_else(|| length(*n, u))?,
            Calc::Key(k) => key(k)?,
            Calc::Sum(v) => v.iter().try_fold(0.0, |acc, c| Some(acc + c.eval(key, length)?))?,
            Calc::Prod(v) => v.iter().try_fold(1.0, |acc, c| Some(acc * c.eval(key, length)?))?,
            Calc::Neg(c) => -c.eval(key, length)?,
            Calc::Inv(c) => 1.0 / c.eval(key, length)?,
            Calc::Func(name, args) => {
                let a = |i: usize| -> Option<f64> { args.get(i)?.eval(key, length) };
                let trig_in = |i: usize| -> Option<f64> {
                    let v = a(i)?;
                    Some(if args[i].ty()? == Ty::ANGLE { v.to_radians() } else { v })
                };
                match name.as_str() {
                    "min" => args.iter().try_fold(f64::INFINITY, |m, c| {
                        let v = c.eval(key, length)?;
                        Some(if v.is_nan() || m.is_nan() { f64::NAN } else { m.min(v) })
                    })?,
                    "max" => args.iter().try_fold(f64::NEG_INFINITY, |m, c| {
                        let v = c.eval(key, length)?;
                        Some(if v.is_nan() || m.is_nan() { f64::NAN } else { m.max(v) })
                    })?,
                    "clamp" => {
                        let lo = if matches!(&args[0], Calc::Key(k) if k == "none") { f64::NEG_INFINITY } else { a(0)? };
                        let v = a(1)?;
                        let hi = if matches!(&args[2], Calc::Key(k) if k == "none") { f64::INFINITY } else { a(2)? };
                        if lo.is_nan() || v.is_nan() || hi.is_nan() {
                            f64::NAN
                        } else {
                            v.min(hi).max(lo)
                        }
                    }
                    "hypot" => args.iter().try_fold(0.0, |s, c| Some(s + c.eval(key, length)?.powi(2)))?.sqrt(),
                    "abs" => a(0)?.abs(),
                    "sign" => {
                        let v = a(0)?;
                        if v.is_nan() || v == 0.0 {
                            v
                        } else {
                            v.signum()
                        }
                    }
                    "sin" => trig_in(0)?.sin(),
                    "cos" => trig_in(0)?.cos(),
                    "tan" => trig_in(0)?.tan(),
                    "asin" => a(0)?.asin().to_degrees(),
                    "acos" => a(0)?.acos().to_degrees(),
                    "atan" => a(0)?.atan().to_degrees(),
                    "atan2" => a(0)?.atan2(a(1)?).to_degrees(),
                    "pow" => a(0)?.powf(a(1)?),
                    "sqrt" => a(0)?.sqrt(),
                    "exp" => a(0)?.exp(),
                    "log" => {
                        let v = a(0)?;
                        match args.len() {
                            2 => v.ln() / a(1)?.ln(),
                            _ => v.ln(),
                        }
                    }
                    "mod" | "rem" => {
                        let (x, y) = (a(0)?, a(1)?);
                        if y == 0.0 || x.is_infinite() {
                            f64::NAN
                        } else if name == "mod" {
                            if y.is_infinite() {
                                if x == 0.0 || x.signum() == y.signum() { x } else { f64::NAN }
                            } else {
                                x - y * (x / y).floor()
                            }
                        } else if y.is_infinite() {
                            x
                        } else {
                            x % y
                        }
                    }
                    n => {
                        let x = a(0)?;
                        let step = if args.len() == 2 { a(1)? } else { 1.0 };
                        if step == 0.0 {
                            return Some(f64::NAN);
                        }
                        let q = x / step;
                        let r = match n {
                            "round-up" => q.ceil(),
                            "round-down" => q.floor(),
                            "round-to-zero" => q.trunc(),
                            _ => (q + 0.5).floor(),
                        };
                        r * step
                    }
                }
            }
        })
    }

    fn numeric(&self) -> Option<(f64, &str)> {
        match self {
            Calc::Num(n) => Some((*n, "")),
            Calc::Pct(n) => Some((*n, "%")),
            Calc::Dim(n, u) => Some((*n, u.as_str())),
            _ => None,
        }
    }

    fn with_value(&self, v: f64) -> Calc {
        match self {
            Calc::Pct(_) => Calc::Pct(v),
            Calc::Dim(_, u) => Calc::Dim(v, u.clone()),
            _ => Calc::Num(v),
        }
    }

    /// Canonical unit for combining: angles become degrees, absolute
    /// lengths px; other units are kept.
    fn canonical(self) -> Calc {
        match self {
            Calc::Dim(v, u) => {
                if let Some(d) = angle_deg(v, &u).filter(|_| u != "deg") {
                    Calc::Dim(d, "deg".into())
                } else if let Some(p) = absolute_px(v, &u).filter(|_| u != "px") {
                    Calc::Dim(p, "px".into())
                } else {
                    Calc::Dim(v, u)
                }
            }
            c => c,
        }
    }

    /// Simplify per CSS Values 4 §10.10 (numeric folding, sorting sums,
    /// numeric coefficients first in products).
    pub fn simplify(self) -> Calc {
        match self {
            Calc::Neg(c) => match c.simplify() {
                Calc::Num(n) => Calc::Num(-n),
                Calc::Pct(n) => Calc::Pct(-n),
                Calc::Dim(n, u) => Calc::Dim(-n, u),
                Calc::Neg(inner) => *inner,
                other => Calc::Neg(Box::new(other)),
            },
            Calc::Inv(c) => match c.simplify() {
                Calc::Num(n) => Calc::Num(1.0 / n),
                Calc::Inv(inner) => *inner,
                other => Calc::Inv(Box::new(other)),
            },
            Calc::Sum(v) => {
                let mut flat = Vec::new();
                for c in v {
                    match c.simplify() {
                        Calc::Sum(inner) => flat.extend(inner),
                        c => flat.push(c.canonical()),
                    }
                }
                let mut out: Vec<Calc> = Vec::new();
                for c in flat {
                    if let Some((n, u)) = c.numeric() {
                        if let Some(existing) = out.iter_mut().find(|e| e.numeric().is_some_and(|(_, eu)| eu == u)) {
                            let (m, _) = existing.numeric().unwrap_or((0.0, ""));
                            *existing = existing.with_value(m + n);
                            continue;
                        }
                    }
                    out.push(c);
                }
                // Numbers, percentages, dimensions by unit, then the rest.
                let rank = |c: &Calc| match c {
                    Calc::Num(_) => (0, String::new()),
                    Calc::Pct(_) => (1, String::new()),
                    Calc::Dim(_, u) => (2, u.clone()),
                    _ => (3, String::new()),
                };
                out.sort_by(|a, b| {
                    let (ra, ua) = rank(a);
                    let (rb, ub) = rank(b);
                    if ra == 3 && rb == 3 { std::cmp::Ordering::Equal } else { ra.cmp(&rb).then(ua.cmp(&ub)) }
                });
                if out.len() == 1 { out.pop().unwrap_or(Calc::Num(0.0)) } else { Calc::Sum(out) }
            }
            Calc::Prod(v) => {
                let mut flat = Vec::new();
                for c in v {
                    match c.simplify() {
                        Calc::Prod(inner) => flat.extend(inner),
                        c => flat.push(c.canonical()),
                    }
                }
                let mut coef = 1.0;
                let mut has_coef = false;
                let mut rest = Vec::new();
                for c in flat {
                    match c {
                        Calc::Num(n) => {
                            coef *= n;
                            has_coef = true;
                        }
                        c => rest.push(c),
                    }
                }
                // A single numeric leaf with a unit absorbs the coefficient.
                let numeric_unit: Vec<usize> = rest.iter().enumerate().filter(|(_, c)| c.numeric().is_some()).map(|(i, _)| i).collect();
                if numeric_unit.len() == 1 && rest.len() == 1 {
                    let c = &rest[0];
                    let (n, _) = c.numeric().unwrap_or((0.0, ""));
                    return c.with_value(n * coef);
                }
                if rest.is_empty() {
                    return Calc::Num(coef);
                }
                // A coefficient folds into the one numeric leaf with a unit,
                // and numeric factors come first.
                let mut out = Vec::new();
                if numeric_unit.len() == 1 {
                    let i = numeric_unit[0];
                    let leaf = rest.remove(i);
                    let (n, _) = leaf.numeric().unwrap_or((0.0, ""));
                    out.push(leaf.with_value(n * coef));
                } else if has_coef && coef != 1.0 || rest.iter().all(|c| matches!(c, Calc::Inv(_))) {
                    out.push(Calc::Num(coef));
                }
                out.extend(rest);
                if out.len() == 1 { out.pop().unwrap_or(Calc::Num(0.0)) } else { Calc::Prod(out) }
            }
            Calc::Func(name, args) => {
                let args: Vec<Calc> = args.into_iter().map(|a| a.simplify().canonical()).collect();
                let units: Vec<Option<(f64, String)>> = args.iter().map(|a| a.numeric().map(|(v, u)| (v, u.to_string()))).collect();
                let resolvable = units.iter().all(|u| u.as_ref().is_some_and(|(_, u)| u.is_empty() || u == "%" || u == "deg" || u == "px"));
                let same_unit = units.windows(2).all(|w| w[0].as_ref().map(|x| &x.1) == w[1].as_ref().map(|x| &x.1));
                let node = Calc::Func(name.clone(), args.clone());
                if resolvable && (same_unit || matches!(name.as_str(), "sign" | "sin" | "cos" | "tan" | "atan2")) {
                    let no_key = |_: &str| None;
                    let no_len = |_: f64, _: &str| None;
                    if let Some(v) = node.eval(&no_key, &no_len) {
                        let ty = node.ty();
                        return match ty {
                            Some(t) if t == Ty::NUMBER => Calc::Num(v),
                            Some(t) if t == Ty::PERCENT => Calc::Pct(v),
                            Some(t) if t == Ty::ANGLE => Calc::Dim(v, "deg".into()),
                            Some(t) if t == Ty::LENGTH => Calc::Dim(v, "px".into()),
                            _ => node,
                        };
                    }
                }
                node
            }
            c => c,
        }
    }

    /// Serialize a simplified tree as the value of a math function.
    pub fn to_css(&self) -> String {
        match self {
            Calc::Func(..) => self.node_css(true),
            _ => format!("calc({})", self.node_css(true)),
        }
    }

    fn node_css(&self, top: bool) -> String {
        match self {
            Calc::Num(n) => fmt_number(*n),
            Calc::Pct(n) => format!("{}%", fmt_number(*n)),
            Calc::Dim(n, u) => format!("{}{u}", fmt_number(*n)),
            Calc::Key(k) => k.clone(),
            Calc::Sum(v) => {
                let mut s = String::new();
                for (i, c) in v.iter().enumerate() {
                    if i == 0 {
                        s.push_str(&c.node_css(false));
                        continue;
                    }
                    match c {
                        Calc::Neg(inner) => {
                            s.push_str(" - ");
                            s.push_str(&inner.node_css(false));
                        }
                        c if c.numeric().is_some_and(|(n, _)| n < 0.0) => {
                            s.push_str(" - ");
                            s.push_str(&c.with_value(-c.numeric().map_or(0.0, |x| x.0)).node_css(false));
                        }
                        c => {
                            s.push_str(" + ");
                            s.push_str(&c.node_css(false));
                        }
                    }
                }
                if top { s } else { format!("({s})") }
            }
            Calc::Prod(v) => {
                let mut s = String::new();
                for (i, c) in v.iter().enumerate() {
                    match c {
                        Calc::Inv(inner) => {
                            if i == 0 {
                                s.push('1');
                            }
                            s.push_str(" / ");
                            s.push_str(&inner.node_css(false));
                        }
                        c => {
                            if i != 0 {
                                s.push_str(" * ");
                            }
                            s.push_str(&c.node_css(false));
                        }
                    }
                }
                if top { s } else { format!("({s})") }
            }
            Calc::Neg(c) => format!("(-1 * {})", c.node_css(false)),
            Calc::Inv(c) => format!("(1 / {})", c.node_css(false)),
            Calc::Func(name, args) => {
                let args: Vec<String> = args.iter().map(|a| a.node_css(true)).collect();
                match name.as_str() {
                    "round-up" => format!("round(up, {})", args.join(", ")),
                    "round-down" => format!("round(down, {})", args.join(", ")),
                    "round-to-zero" => format!("round(to-zero, {})", args.join(", ")),
                    n => format!("{n}({})", args.join(", ")),
                }
            }
        }
    }
}

/// A number as CSS serializes it: up to six decimals, no exponent.
pub fn fmt_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "infinity".into() } else { "-infinity".into() };
    }
    let r = (n * 1e6).round() / 1e6;
    if r == 0.0 {
        return "0".into();
    }
    let s = format!("{r:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_css::Tokenizer;

    fn css(s: &str, keys: &[&str]) -> String {
        let t = Tokenizer::tokenize(s);
        parse(&t, keys).unwrap().simplify().to_css()
    }

    #[test]
    fn simplified_serialization() {
        assert_eq!(css("calc(r)", &["r"]), "calc(r)");
        assert_eq!(css("calc(g * 2)", &["g"]), "calc(2 * g)");
        assert_eq!(css("calc(r * .5)", &["r"]), "calc(0.5 * r)");
        assert_eq!(css("calc(g * .5 + g * .5)", &["g"]), "calc((0.5 * g) + (0.5 * g))");
        assert_eq!(css("calc(l - 20)", &["l"]), "calc(-20 + l)");
        assert_eq!(css("calc(a / 3)", &["a"]), "calc(0.333333 * a)");
        assert_eq!(css("calc(1 / l)", &["l"]), "calc(1 / l)");
        assert_eq!(css("calc(50 * 3)", &[]), "calc(150)");
        assert_eq!(css("calc(0.5 - 1)", &[]), "calc(-0.5)");
        assert_eq!(css("calc(-150% / 3)", &[]), "calc(-50%)");
        assert_eq!(css("calc(0 / 0)", &[]), "calc(NaN)");
        assert_eq!(css("clamp(50%, 80%, 70%)", &[]), "calc(70%)");
        assert_eq!(css("min(0.5, 0)", &[]), "calc(0)");
        assert_eq!(css("calc(50 + (sign(1em - 10px) * 10))", &[]), "calc(50 + (10 * sign(1em - 10px)))");
        assert!(parse(&Tokenizer::tokenize("calc(r + 1%)"), &["r"]).is_none());
        assert!(parse(&Tokenizer::tokenize("calc(r+1)"), &["r"]).is_none());
    }

    #[test]
    fn evaluation() {
        let t = Tokenizer::tokenize("calc((sin(l * (50rad / 50)) + 1) * 50)");
        let c = parse(&t, &["l"]).unwrap();
        let v = c.eval(&|_| Some(50.0), &|_, _| None).unwrap();
        assert!((v - ((50f64).sin() + 1.0) * 50.0).abs() < 1e-9);
    }
}
