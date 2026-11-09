//! CSS parsing: tokenizer, selectors and a stylesheet/declaration parser
//! with `@media` evaluation and `@import` discovery.

pub mod selector;
pub mod tokenizer;

pub use selector::{matches, Selector, Specificity};
pub use tokenizer::{Token, Tokenizer};

/// A parsed declaration. `value` is the raw component-value token list with
/// leading/trailing whitespace trimmed; property handlers interpret it.
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Vec<Token>,
    pub important: bool,
}

#[derive(Debug, Clone)]
pub struct StyleRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone)]
pub enum Rule {
    Style(StyleRule),
    Media {
        query: MediaQuery,
        rules: Vec<Rule>,
    },
    /// `@import url(...) media;` — the engine resolves and inlines these.
    Import {
        href: String,
        query: MediaQuery,
    },
    /// `@font-face`, `@keyframes`, `@supports` (evaluated as true when the
    /// condition uses properties we know), etc.
    Supports {
        matches: bool,
        rules: Vec<Rule>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

/// Simplified media query: a disjunction of conjunctions of features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaQuery {
    pub alternatives: Vec<MediaAlternative>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaAlternative {
    pub negated: bool,
    /// `screen`, `all`, `print`, ... (None == all)
    pub media_type: Option<String>,
    pub features: Vec<MediaFeature>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaFeature {
    MinWidth(f32),
    MaxWidth(f32),
    MinHeight(f32),
    MaxHeight(f32),
    /// `(orientation: landscape)` etc. — stored as (name, value)
    Other(String, String),
    /// Something we could not understand; evaluates to false.
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct MediaEnv {
    pub width: f32,
    pub height: f32,
    pub screen: bool,
}

impl MediaQuery {
    pub fn always() -> Self {
        MediaQuery { alternatives: vec![] }
    }

    pub fn evaluate(&self, env: &MediaEnv) -> bool {
        if self.alternatives.is_empty() {
            return true;
        }
        self.alternatives.iter().any(|a| a.evaluate(env))
    }
}

impl MediaAlternative {
    fn evaluate(&self, env: &MediaEnv) -> bool {
        let type_ok = match self.media_type.as_deref() {
            None | Some("all") => true,
            Some("screen") => env.screen,
            Some("print") => !env.screen,
            Some(_) => false,
        };
        let features_ok = self.features.iter().all(|f| match f {
            MediaFeature::MinWidth(w) => env.width >= *w,
            MediaFeature::MaxWidth(w) => env.width <= *w,
            MediaFeature::MinHeight(h) => env.height >= *h,
            MediaFeature::MaxHeight(h) => env.height <= *h,
            MediaFeature::Other(name, value) => match (name.as_str(), value.as_str()) {
                ("orientation", "landscape") => env.width >= env.height,
                ("orientation", "portrait") => env.width < env.height,
                ("prefers-color-scheme", "light") => true,
                ("prefers-color-scheme", "dark") => false,
                ("prefers-reduced-motion", _) => true,
                ("hover", "hover") => true,
                ("pointer", "fine") => true,
                ("color", _) => true,
                ("min-resolution", _) | ("max-resolution", _) | ("-webkit-min-device-pixel-ratio", _) => false,
                _ => false,
            },
            MediaFeature::Unknown => false,
        });
        let result = type_ok && features_ok;
        if self.negated {
            !result
        } else {
            result
        }
    }
}

impl Stylesheet {
    /// Collect the style rules that apply under `env`, in source order,
    /// flattening `@media` / `@supports` groups.
    pub fn active_rules<'a>(&'a self, env: &MediaEnv, out: &mut Vec<&'a StyleRule>) {
        fn walk<'a>(rules: &'a [Rule], env: &MediaEnv, out: &mut Vec<&'a StyleRule>) {
            for r in rules {
                match r {
                    Rule::Style(s) => out.push(s),
                    Rule::Media { query, rules } => {
                        if query.evaluate(env) {
                            walk(rules, env, out);
                        }
                    }
                    Rule::Supports { matches, rules } => {
                        if *matches {
                            walk(rules, env, out);
                        }
                    }
                    Rule::Import { .. } => {}
                }
            }
        }
        walk(&self.rules, env, out);
    }

    pub fn imports(&self) -> Vec<(String, MediaQuery)> {
        self.rules
            .iter()
            .filter_map(|r| match r {
                Rule::Import { href, query } => Some((href.clone(), query.clone())),
                _ => None,
            })
            .collect()
    }
}

// ---------------------------------------------------------------- parser

pub fn parse_stylesheet(src: &str) -> Stylesheet {
    let toks = Tokenizer::tokenize(src);
    let mut p = Parser { toks: &toks, pos: 0 };
    Stylesheet {
        rules: p.parse_rules(true),
    }
}

/// Parse the contents of a `style="..."` attribute.
pub fn parse_declaration_list(src: &str) -> Vec<Declaration> {
    let toks = Tokenizer::tokenize(src);
    let mut p = Parser { toks: &toks, pos: 0 };
    p.parse_declarations()
}

/// Parse a media query list such as `screen and (max-width: 600px), print`.
pub fn parse_media_query(src: &str) -> MediaQuery {
    let toks = Tokenizer::tokenize(src);
    parse_media_tokens(&toks)
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.toks.get(self.pos)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(Token::Whitespace) | Some(Token::Cdo) | Some(Token::Cdc)) {
            self.pos += 1;
        }
    }

    /// Consume tokens up to (not including) `}` at depth 0 or `;`/`{` as
    /// requested. Returns the slice consumed.
    fn consume_until(&mut self, stop_semicolon: bool, stop_brace: bool) -> &'a [Token] {
        let start = self.pos;
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            match t {
                Token::OpenParen | Token::Function(_) | Token::OpenBracket => depth += 1,
                Token::CloseParen | Token::CloseBracket => depth -= 1,
                Token::OpenBrace if depth <= 0 && stop_brace => break,
                Token::OpenBrace => depth += 1,
                Token::CloseBrace if depth <= 0 => break,
                Token::CloseBrace => depth -= 1,
                Token::Semicolon if depth <= 0 && stop_semicolon => break,
                _ => {}
            }
            self.pos += 1;
        }
        &self.toks[start..self.pos]
    }

    /// Consume a `{ ... }` block and return the tokens inside.
    fn consume_block(&mut self) -> &'a [Token] {
        debug_assert_eq!(self.peek(), Some(&Token::OpenBrace));
        self.pos += 1;
        let start = self.pos;
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            match t {
                Token::OpenBrace => depth += 1,
                Token::CloseBrace => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.pos += 1;
        }
        let inner = &self.toks[start..self.pos];
        if self.peek() == Some(&Token::CloseBrace) {
            self.pos += 1;
        }
        inner
    }

    fn parse_rules(&mut self, top_level: bool) -> Vec<Rule> {
        let mut rules = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(Token::CloseBrace) => {
                    if top_level {
                        self.pos += 1;
                        continue;
                    }
                    break;
                }
                Some(Token::AtKeyword(name)) => {
                    let name = name.to_ascii_lowercase();
                    self.pos += 1;
                    let prelude = self.consume_until(true, true);
                    match self.peek() {
                        Some(Token::OpenBrace) => {
                            let inner = self.consume_block();
                            match name.as_str() {
                                "media" => {
                                    let query = parse_media_tokens(prelude);
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    rules.push(Rule::Media {
                                        query,
                                        rules: sub.parse_rules(false),
                                    });
                                }
                                "supports" => {
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    rules.push(Rule::Supports {
                                        matches: evaluate_supports(prelude),
                                        rules: sub.parse_rules(false),
                                    });
                                }
                                "layer" | "container" | "scope" => {
                                    // Treat as transparent grouping (layers lose their ordering semantics).
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    rules.extend(sub.parse_rules(false));
                                }
                                _ => {} // font-face, keyframes, page, ...
                            }
                        }
                        _ => {
                            // Statement at-rule
                            if self.peek() == Some(&Token::Semicolon) {
                                self.pos += 1;
                            }
                            if name == "import" {
                                if let Some(imp) = parse_import(prelude) {
                                    rules.push(imp);
                                }
                            }
                        }
                    }
                }
                Some(_) => {
                    let prelude = self.consume_until(false, true);
                    if self.peek() != Some(&Token::OpenBrace) {
                        // Garbage until EOF
                        break;
                    }
                    let inner = self.consume_block();
                    let mut sp = selector::SelectorParser::new(prelude);
                    if let Some(selectors) = sp.parse_list() {
                        let mut dp = Parser { toks: inner, pos: 0 };
                        let declarations = dp.parse_declarations();
                        if !declarations.is_empty() {
                            rules.push(Rule::Style(StyleRule {
                                selectors,
                                declarations,
                            }));
                        }
                    }
                }
            }
        }
        rules
    }

    fn parse_declarations(&mut self) -> Vec<Declaration> {
        let mut decls = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(Token::Semicolon) => {
                    self.pos += 1;
                    continue;
                }
                Some(Token::Ident(name)) => {
                    let name = name.to_ascii_lowercase();
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() != Some(&Token::Colon) {
                        self.consume_until(true, false);
                        continue;
                    }
                    self.pos += 1;
                    let value = self.consume_until(true, false);
                    if self.peek() == Some(&Token::Semicolon) {
                        self.pos += 1;
                    }
                    let mut value: Vec<Token> = value.to_vec();
                    // trim whitespace
                    while value.first() == Some(&Token::Whitespace) {
                        value.remove(0);
                    }
                    while value.last() == Some(&Token::Whitespace) {
                        value.pop();
                    }
                    // !important
                    let mut important = false;
                    let n = value.len();
                    if n >= 2 {
                        if let (Token::Delim('!'), Token::Ident(i)) = (&value[n - 2], &value[n - 1]) {
                            if i.eq_ignore_ascii_case("important") {
                                important = true;
                                value.truncate(n - 2);
                                while value.last() == Some(&Token::Whitespace) {
                                    value.pop();
                                }
                            }
                        }
                    }
                    if value.is_empty() {
                        continue;
                    }
                    decls.push(Declaration {
                        name,
                        value,
                        important,
                    });
                }
                Some(_) => {
                    // Skip malformed declaration (or nested rule) up to ';' or a block
                    self.consume_until(true, true);
                    if self.peek() == Some(&Token::OpenBrace) {
                        self.consume_block();
                    } else if self.peek() == Some(&Token::Semicolon) {
                        self.pos += 1;
                    }
                }
            }
        }
        decls
    }
}

fn parse_import(prelude: &[Token]) -> Option<Rule> {
    let mut it = prelude.iter().filter(|t| **t != Token::Whitespace);
    let href = match it.next()? {
        Token::String(s) => s.clone(),
        Token::Url(u) => u.clone(),
        Token::Function(f) if f.eq_ignore_ascii_case("url") => match it.next()? {
            Token::String(s) => {
                // skip ')'
                it.next();
                s.clone()
            }
            _ => return None,
        },
        _ => return None,
    };
    let rest: Vec<Token> = it.cloned().collect();
    // Skip `layer(...)`/`supports(...)` prefixes.
    let mut i = 0;
    while i < rest.len() {
        match &rest[i] {
            Token::Function(f) if f.eq_ignore_ascii_case("layer") || f.eq_ignore_ascii_case("supports") => {
                let mut depth = 1;
                i += 1;
                while i < rest.len() && depth > 0 {
                    match rest[i] {
                        Token::Function(_) | Token::OpenParen => depth += 1,
                        Token::CloseParen => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            Token::Ident(f) if f.eq_ignore_ascii_case("layer") => i += 1,
            _ => break,
        }
    }
    let query = parse_media_tokens(&rest[i..]);
    Some(Rule::Import { href, query })
}

fn evaluate_supports(prelude: &[Token]) -> bool {
    // We claim support for everything except a few properties we know we
    // don't implement, so `@supports (display: grid)` style guards are
    // honoured conservatively.
    let mut s = String::new();
    for t in prelude {
        match t {
            Token::Ident(i) => {
                s.push_str(i);
                s.push(' ');
            }
            Token::Function(f) => {
                s.push_str(f);
                s.push('(');
            }
            _ => {}
        }
    }
    let s = s.to_ascii_lowercase();
    let negated = s.trim_start().starts_with("not ");
    let unsupported = s.contains("grid") || s.contains("selector(") || s.contains("container") || s.contains("backdrop-filter");
    if negated {
        unsupported
    } else {
        !unsupported
    }
}

fn parse_media_tokens(toks: &[Token]) -> MediaQuery {
    let toks: Vec<&Token> = toks.iter().filter(|t| **t != Token::Whitespace).collect();
    if toks.is_empty() {
        return MediaQuery::always();
    }
    let mut alternatives = Vec::new();
    for part in toks.split(|t| **t == Token::Comma) {
        let mut alt = MediaAlternative::default();
        let mut i = 0;
        let mut valid = true;
        while i < part.len() {
            match part[i] {
                Token::Ident(w) => {
                    let w = w.to_ascii_lowercase();
                    match w.as_str() {
                        "not" => alt.negated = !alt.negated,
                        "only" | "and" => {}
                        "or" => {
                            valid = false;
                        }
                        _ => alt.media_type = Some(w),
                    }
                    i += 1;
                }
                Token::OpenParen => {
                    // ( name [: value] )
                    let mut j = i + 1;
                    let mut depth = 1;
                    while j < part.len() && depth > 0 {
                        match part[j] {
                            Token::OpenParen | Token::Function(_) => depth += 1,
                            Token::CloseParen => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    let inner = &part[i + 1..j.saturating_sub(1).max(i + 1)];
                    alt.features.push(parse_media_feature(inner));
                    i = j;
                }
                _ => {
                    valid = false;
                    i += 1;
                }
            }
        }
        if !valid {
            alt.features.push(MediaFeature::Unknown);
        }
        alternatives.push(alt);
    }
    MediaQuery { alternatives }
}

fn parse_media_feature(inner: &[&Token]) -> MediaFeature {
    // Forms: (name), (name: value), (value <= name <= value), (name >= value)
    let name = match inner.first() {
        Some(Token::Ident(n)) => n.to_ascii_lowercase(),
        Some(Token::Number(_)) | Some(Token::Dimension(..)) => {
            // range syntax e.g. (400px <= width <= 700px)
            return parse_range_feature(inner);
        }
        _ => return MediaFeature::Unknown,
    };
    if inner.len() == 1 {
        return MediaFeature::Other(name, String::new());
    }
    match inner.get(1) {
        Some(Token::Colon) => {
            let value = &inner[2..];
            match (name.as_str(), value.first()) {
                ("min-width", Some(Token::Dimension(v, u))) => MediaFeature::MinWidth(to_px(*v, u)),
                ("max-width", Some(Token::Dimension(v, u))) => MediaFeature::MaxWidth(to_px(*v, u)),
                ("min-height", Some(Token::Dimension(v, u))) => MediaFeature::MinHeight(to_px(*v, u)),
                ("max-height", Some(Token::Dimension(v, u))) => MediaFeature::MaxHeight(to_px(*v, u)),
                ("min-width" | "max-width" | "min-height" | "max-height", Some(Token::Number(v))) if *v == 0.0 => {
                    if name.starts_with("min") {
                        MediaFeature::MinWidth(0.0)
                    } else {
                        MediaFeature::MaxWidth(0.0)
                    }
                }
                (_, Some(Token::Ident(v))) => MediaFeature::Other(name, v.to_ascii_lowercase()),
                (_, Some(Token::Number(v))) => MediaFeature::Other(name, format!("{v}")),
                (_, Some(Token::Dimension(v, u))) => MediaFeature::Other(name, format!("{v}{u}")),
                _ => MediaFeature::Unknown,
            }
        }
        Some(Token::Delim(c @ ('<' | '>'))) => {
            // (width >= 600px) / (width < 600px)
            let mut k = 2;
            let mut inclusive = false;
            if let Some(Token::Delim('=')) = inner.get(k) {
                inclusive = true;
                k += 1;
            }
            let (v, u) = match inner.get(k) {
                Some(Token::Dimension(v, u)) => (*v, u.as_str()),
                _ => return MediaFeature::Unknown,
            };
            let px = to_px(v, u) + if inclusive { 0.0 } else if *c == '>' { 0.01 } else { -0.01 };
            match (name.as_str(), c) {
                ("width", '>') => MediaFeature::MinWidth(px),
                ("width", '<') => MediaFeature::MaxWidth(px),
                ("height", '>') => MediaFeature::MinHeight(px),
                ("height", '<') => MediaFeature::MaxHeight(px),
                _ => MediaFeature::Unknown,
            }
        }
        _ => MediaFeature::Unknown,
    }
}

fn parse_range_feature(inner: &[&Token]) -> MediaFeature {
    // (400px <= width <= 700px) -> we only keep the first bound; good enough.
    let (v, u) = match inner.first() {
        Some(Token::Dimension(v, u)) => (*v, u.as_str()),
        _ => return MediaFeature::Unknown,
    };
    let op = match inner.get(1) {
        Some(Token::Delim(c)) => *c,
        _ => return MediaFeature::Unknown,
    };
    let name = inner.iter().find_map(|t| match t {
        Token::Ident(n) => Some(n.to_ascii_lowercase()),
        _ => None,
    });
    let px = to_px(v, u);
    match (name.as_deref(), op) {
        (Some("width"), '<') => MediaFeature::MinWidth(px),
        (Some("width"), '>') => MediaFeature::MaxWidth(px),
        (Some("height"), '<') => MediaFeature::MinHeight(px),
        (Some("height"), '>') => MediaFeature::MaxHeight(px),
        _ => MediaFeature::Unknown,
    }
}

fn to_px(v: f32, unit: &str) -> f32 {
    match unit {
        "px" => v,
        "em" | "rem" => v * 16.0,
        "pt" => v * 4.0 / 3.0,
        "cm" => v * 37.8,
        "mm" => v * 3.78,
        "in" => v * 96.0,
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sheet() {
        let css = "@import url(a.css) screen; @charset 'utf-8'; body { color: red; margin: 0 !important } \
                   @media (max-width: 600px) { .x { display: none } } p:hover, p { font: 12px/1.5 serif } \
                   @font-face { font-family: X; src: url(x.woff) } bad { ; } a { }";
        let sheet = parse_stylesheet(css);
        assert_eq!(sheet.imports().len(), 1);
        let env = MediaEnv { width: 500.0, height: 800.0, screen: true };
        let mut rules = Vec::new();
        sheet.active_rules(&env, &mut rules);
        assert_eq!(rules.len(), 3, "{:?}", rules);
        assert!(rules[0].declarations[1].important);
        let env2 = MediaEnv { width: 900.0, height: 800.0, screen: true };
        let mut rules2 = Vec::new();
        sheet.active_rules(&env2, &mut rules2);
        assert_eq!(rules2.len(), 2);
    }

    #[test]
    fn style_attr() {
        let d = parse_declaration_list("color: red; width:10px");
        assert_eq!(d.len(), 2);
        assert_eq!(d[1].value, vec![Token::Dimension(10.0, "px".into())]);
    }

    #[test]
    fn media_queries() {
        let q = parse_media_query("screen and (min-width: 600px) and (max-width: 900px), print");
        let env = MediaEnv { width: 700.0, height: 800.0, screen: true };
        assert!(q.evaluate(&env));
        let env = MediaEnv { width: 1000.0, height: 800.0, screen: true };
        assert!(!q.evaluate(&env));
        let q = parse_media_query("not screen");
        assert!(!q.evaluate(&env));
        let q = parse_media_query("(width >= 600px)");
        assert!(q.evaluate(&env));
    }
}
