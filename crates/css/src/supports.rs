//! `@supports` conditions (CSS Conditional 3/4): `not`, `and`, `or`,
//! parentheses, `(property: value)` declarations, `selector()`,
//! `font-tech()` / `font-format()`.
//!
//! A declaration is supported unless its property (or a value keyword it
//! depends on) is one we know we do not implement; a selector is
//! supported when it parses without unknown pseudo-classes or elements.

use crate::selector::{Pseudo, SelectorParser};
use crate::tokenizer::Token;

/// Properties (and `property: value` pairs) the engine does not implement:
/// guards that test them must take the fallback branch.
const UNSUPPORTED_PROPERTIES: &[&str] = &[
    "anchor-name", "position-anchor", "position-area", "inset-area", "position-try", "position-try-fallbacks", "position-visibility", "anchor-scope",
    "view-transition-name", "view-transition-class", "animation-timeline", "scroll-timeline", "scroll-timeline-name", "view-timeline", "view-timeline-name",
    "animation-range", "timeline-scope", "initial-letter", "-webkit-initial-letter", "math-depth", "math-shift", "reading-flow", "reading-order",
    "interpolate-size", "field-sizing", "text-box", "text-box-trim", "text-box-edge", "hanging-punctuation", "box-decoration-break", "-webkit-box-reflect",
    "shape-outside", "offset-path", "offset-distance", "offset", "zoom", "corner-shape", "dynamic-range-limit", "overlay", "container-name-x",
];

/// Values we do not implement for otherwise supported properties.
const UNSUPPORTED_VALUES: &[(&str, &str)] = &[
    ("grid-template-columns", "subgrid"),
    ("grid-template-rows", "subgrid"),
    ("display", "masonry"),
    ("display", "ruby"),
    ("display", "math"),
    ("grid-template-rows", "masonry"),
    ("grid-template-columns", "masonry"),
    ("text-wrap", "pretty"),
    ("text-wrap-style", "pretty"),
    ("transform-style", "preserve-3d"),
    ("mix-blend-mode", "plus-darker"),
];

/// Checks `property: value` against the property grammars, which live in
/// the style crate: `Some(false)` when the property is unknown or the
/// value does not parse, `None` when it cannot tell.
pub type DeclarationValidator = fn(&str, &[Token]) -> Option<bool>;

static VALIDATOR: std::sync::OnceLock<DeclarationValidator> = std::sync::OnceLock::new();

/// Install the declaration validator (first call wins).
pub fn set_declaration_validator(f: DeclarationValidator) {
    let _ = VALIDATOR.set(f);
}

/// Evaluate the prelude of an `@supports` rule.
pub fn evaluate(prelude: &[Token]) -> bool {
    let toks: Vec<&Token> = prelude.iter().filter(|t| !matches!(t, Token::Whitespace)).collect();
    let mut p = Cond { toks: &toks, pos: 0 };
    let r = p.condition();
    r.unwrap_or(false) && p.pos == toks.len()
}

struct Cond<'a, 'b> {
    toks: &'b [&'a Token],
    pos: usize,
}

impl<'a, 'b> Cond<'a, 'b> {
    fn peek_ident(&self) -> Option<String> {
        match self.toks.get(self.pos) {
            Some(Token::Ident(s)) => Some(s.to_ascii_lowercase()),
            _ => None,
        }
    }

    /// `not <in-parens>` | `<in-parens> [and <in-parens>]*` | `... [or ...]*`
    fn condition(&mut self) -> Option<bool> {
        if self.peek_ident().as_deref() == Some("not") {
            self.pos += 1;
            return self.in_parens().map(|v| !v);
        }
        let first = self.in_parens()?;
        match self.peek_ident().as_deref() {
            Some("and") => {
                let mut v = first;
                while self.peek_ident().as_deref() == Some("and") {
                    self.pos += 1;
                    v &= self.in_parens()?;
                }
                Some(v)
            }
            Some("or") => {
                let mut v = first;
                while self.peek_ident().as_deref() == Some("or") {
                    self.pos += 1;
                    v |= self.in_parens()?;
                }
                Some(v)
            }
            _ => Some(first),
        }
    }

    /// The tokens up to the parenthesis closing the one just consumed.
    fn group(&mut self) -> Option<Vec<&'a Token>> {
        let mut depth = 1;
        let start = self.pos;
        while self.pos < self.toks.len() {
            match self.toks[self.pos] {
                Token::OpenParen | Token::Function(_) => depth += 1,
                Token::CloseParen => {
                    depth -= 1;
                    if depth == 0 {
                        let inner = self.toks[start..self.pos].to_vec();
                        self.pos += 1;
                        return Some(inner);
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        None
    }

    fn in_parens(&mut self) -> Option<bool> {
        match self.toks.get(self.pos)? {
            Token::OpenParen => {
                self.pos += 1;
                let inner = self.group()?;
                // A nested condition or a declaration.
                let is_decl = matches!(inner.as_slice(), [Token::Ident(_), Token::Colon, ..]);
                if is_decl {
                    return Some(declaration(&inner));
                }
                let mut sub = Cond { toks: &inner, pos: 0 };
                let v = sub.condition();
                // Unknown syntax inside parentheses is false, not an error.
                Some(v.unwrap_or(false) && sub.pos == inner.len())
            }
            Token::Function(f) => {
                let f = f.to_ascii_lowercase();
                self.pos += 1;
                let inner = self.group()?;
                Some(match f.as_str() {
                    "selector" => selector_supported(&inner),
                    "font-format" => single_ident(&inner).is_some_and(|f| matches!(f.as_str(), "woff" | "woff2" | "truetype" | "opentype" | "collection")),
                    "font-tech" => single_ident(&inner).is_some_and(|f| matches!(f.as_str(), "features-opentype" | "color-colrv0" | "color-cbdt" | "color-sbix")),
                    "at-rule" => at_rule_supported(&inner),
                    _ => false,
                })
            }
            _ => None,
        }
    }
}

/// Split `!important` off a declaration value; `None` when the value has
/// another `!` (an invalid declaration).
fn strip_important<'a, 'b>(value: &'b [&'a Token]) -> Option<&'b [&'a Token]> {
    match value.iter().position(|t| matches!(t, Token::Delim('!'))) {
        None => Some(value),
        Some(i) => match &value[i + 1..] {
            [Token::Ident(s)] if s.eq_ignore_ascii_case("important") => Some(&value[..i]),
            _ => None,
        },
    }
}

/// A declaration value is never valid with a top-level `;`, unbalanced
/// brackets or bad tokens.
fn well_formed(value: &[&Token]) -> bool {
    let mut stack = Vec::new();
    for t in value {
        match t {
            Token::Semicolon | Token::BadString | Token::BadUrl => return false,
            Token::OpenParen | Token::Function(_) => stack.push(')'),
            Token::OpenBracket => stack.push(']'),
            Token::OpenBrace => stack.push('}'),
            Token::CloseParen | Token::CloseBracket | Token::CloseBrace => {
                let want = match t {
                    Token::CloseParen => ')',
                    Token::CloseBracket => ']',
                    _ => '}',
                };
                if stack.pop() != Some(want) {
                    return false;
                }
            }
            _ => {}
        }
    }
    stack.is_empty()
}

fn declaration(toks: &[&Token]) -> bool {
    let Some(Token::Ident(name)) = toks.first() else { return false };
    let name = name.to_ascii_lowercase();
    let Some(value) = strip_important(&toks[2..]) else { return false };
    if !well_formed(value) {
        return false;
    }
    if name.starts_with("--") {
        return true;
    }
    if value.is_empty() {
        return false;
    }
    if UNSUPPORTED_PROPERTIES.contains(&name.as_str()) {
        return false;
    }
    let values: Vec<String> = toks[2..]
        .iter()
        .filter_map(|t| match t {
            Token::Ident(s) => Some(s.to_ascii_lowercase()),
            Token::Function(f) => Some(format!("{}(", f.to_ascii_lowercase())),
            _ => None,
        })
        .collect();
    if UNSUPPORTED_VALUES.iter().any(|(p, v)| *p == name && values.iter().any(|x| x == v)) {
        return false;
    }
    // Value functions we do not implement anywhere.
    if values.iter().any(|v| matches!(v.as_str(), "anchor(" | "anchor-size(" | "sibling-index(" | "sibling-count(" | "if(" | "random(" | "progress(")) {
        return false;
    }
    // Vendor-prefixed properties of other engines.
    if name.starts_with("-moz-") || name.starts_with("-ms-") || name.starts_with("-o-") {
        return false;
    }
    match VALIDATOR.get() {
        Some(validate) => {
            let owned: Vec<Token> = value.iter().map(|t| (*t).clone()).collect();
            validate(&name, &owned).unwrap_or(true)
        }
        None => true,
    }
}

/// A selector parsed with an unknown pseudo-class or pseudo-element
/// somewhere, including inside the forgiving lists of `:is()` and alike.
fn has_unknown(sel: &crate::selector::Selector) -> bool {
    sel.compounds.iter().any(|c| {
        c.pseudos.iter().any(|p| match p {
            Pseudo::Never => true,
            Pseudo::Not(l) | Pseudo::Is(l) | Pseudo::Where(l) | Pseudo::Host(l) | Pseudo::HostContext(l) | Pseudo::Slotted(l) => {
                l.iter().any(has_unknown)
            }
            Pseudo::NthChildOf(_, _, l) | Pseudo::NthLastChildOf(_, _, l) => l.iter().any(has_unknown),
            Pseudo::Has(l) => l.iter().any(|(_, s)| has_unknown(s)),
            _ => false,
        })
    })
}

/// `selector(<complex-selector>)`: one selector (not a list) that parses
/// with known pseudo-classes and pseudo-elements only.
fn selector_supported(toks: &[&Token]) -> bool {
    let owned: Vec<Token> = toks.iter().map(|t| (*t).clone()).collect();
    let mut p = SelectorParser::new(&owned);
    match p.parse_list() {
        Some(list) => list.len() == 1 && !list.iter().any(has_unknown),
        None => false,
    }
}

/// The single identifier argument of `font-format()` / `font-tech()`.
fn single_ident(toks: &[&Token]) -> Option<String> {
    match toks {
        [Token::Ident(s)] => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

/// `at-rule(<at-keyword>)`: at-rules the parser knows.
fn at_rule_supported(toks: &[&Token]) -> bool {
    match toks {
        [Token::AtKeyword(k)] => matches!(
            k.to_ascii_lowercase().as_str(),
            "media" | "supports" | "import" | "namespace" | "charset" | "font-face" | "keyframes" | "-webkit-keyframes" | "layer"
                | "container" | "scope" | "page" | "property" | "font-feature-values" | "counter-style" | "starting-style"
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::evaluate;
    use crate::tokenizer::Tokenizer;

    fn ok(s: &str) -> bool {
        evaluate(&Tokenizer::tokenize(s))
    }

    #[test]
    fn conditions() {
        assert!(ok("(display: grid)"));
        assert!(ok("(display: flex) and (gap: 1rem)"));
        assert!(!ok("not (display: grid)"));
        assert!(ok("(display: grid) or (anchor-name: --a)"));
        assert!(!ok("(anchor-name: --a)"));
        assert!(!ok("(grid-template-columns: subgrid)"));
        assert!(ok("selector(:has(a))"));
        assert!(ok("selector(:focus-visible)"));
        assert!(!ok("selector(:totally-unknown)"));
        assert!(ok("not selector(:totally-unknown)"));
        assert!(ok("((display: grid))"));
        assert!(!ok("(-moz-appearance: none)"));
        assert!(ok("(--custom: 1)"));
        assert!(!ok("display: grid"));
        assert!(!ok("(margin: 0;)"));
        assert!(!ok("(margin: 0])"));
        assert!(ok("(--foo: whatever !important)"));
        assert!(!ok("(--foo: whatever !bogus)"));
        assert!(ok("(--foo:)"));
        assert!(!ok("selector(div, div)"));
        assert!(!ok("selector(:is(:foo, .a))"));
        assert!(ok("selector(:is(.a))"));
        assert!(ok("font-format(opentype)"));
        assert!(!ok("font-format(truetype opentype)"));
        assert!(!ok("font-format(\"woff\")"));
        assert!(!ok("font-tech(features-opentype, color-COLRv0)"));
        assert!(ok("at-rule(@supports) and at-rule( @media )"));
        assert!(ok("at-rule(@support\\73)"));
        assert!(!ok("at-rule(@foo)"));
    }
}
