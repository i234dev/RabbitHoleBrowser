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
                    "font-format" => inner.iter().any(|t| matches!(t, Token::Ident(s) if matches!(s.to_ascii_lowercase().as_str(), "woff" | "woff2" | "truetype" | "opentype" | "collection"))),
                    "font-tech" => inner.iter().any(|t| matches!(t, Token::Ident(s) if matches!(s.to_ascii_lowercase().as_str(), "features-opentype" | "color-colrv0" | "color-cbdt" | "color-sbix"))),
                    _ => false,
                })
            }
            _ => None,
        }
    }
}

fn declaration(toks: &[&Token]) -> bool {
    let Some(Token::Ident(name)) = toks.first() else { return false };
    let name = name.to_ascii_lowercase();
    if toks.len() < 3 {
        return false;
    }
    if name.starts_with("--") {
        return true;
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
    if values.iter().any(|v| matches!(v.as_str(), "anchor(" | "anchor-size(" | "sibling-index(" | "sibling-count(" | "if(" | "random(" | "progress(" | "contrast-color(")) {
        return false;
    }
    // Vendor-prefixed properties of other engines.
    !(name.starts_with("-moz-") || name.starts_with("-ms-") || name.starts_with("-o-"))
}

fn selector_supported(toks: &[&Token]) -> bool {
    let owned: Vec<Token> = toks.iter().map(|t| (*t).clone()).collect();
    let mut p = SelectorParser::new(&owned);
    match p.parse_list() {
        Some(list) => !list.iter().any(|s| s.compounds.iter().any(|c| c.pseudos.iter().any(|p| matches!(p, Pseudo::Never)))),
        None => false,
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
    }
}
