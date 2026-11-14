//! CSS parsing: tokenizer, selectors and a stylesheet/declaration parser
//! with `@media` evaluation and `@import` discovery.

pub mod selector;
pub mod supports;
pub mod tokenizer;

pub use selector::{matches, matches_with, MatchState, PseudoElement, Selector, Specificity};
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
    /// Cascade layer (`@layer`, dotted for nested layers); `None` for
    /// unlayered rules. A rule without selectors only records a layer's
    /// place in the layer order (`@layer a, b;`).
    pub layer: Option<std::sync::Arc<str>>,
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
    /// `@font-face { ... }` descriptors (font-family, src, font-weight, ...).
    FontFace(Vec<Declaration>),
    /// `@keyframes name { from {..} 50% {..} to {..} }`
    Keyframes {
        name: String,
        frames: Vec<Keyframe>,
    },
}

#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Offsets in 0.0..=1.0 (`from` = 0, `to` = 1, percentages / 100).
    pub offsets: Vec<f32>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// Absolute URL of the sheet, used to resolve `url()` values and
    /// `@import`s. `None` for inline `<style>` (document base applies).
    pub base_url: Option<String>,
    /// Shadow roots this sheet is scoped to (`<style>` inside a shadow tree
    /// or adopted by a shadow root). Empty: a document stylesheet.
    pub shadow_scopes: Vec<usize>,
    /// Identity of the sheet's source (text / URL, media, base) assigned by
    /// the embedder when it collects sheets; 0 = unknown. Lets the style
    /// engine tell which sheets changed between two collections.
    pub source_id: u64,
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
    /// Scripting is enabled (`@media (scripting: enabled)`).
    pub scripting: bool,
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
            MediaFeature::Other(name, value) if is_numeric_feature(name) => numeric_feature(name, value),
            MediaFeature::Other(name, value) => match (name.as_str(), value.as_str()) {
                ("orientation", "landscape") => env.width >= env.height,
                ("orientation", "portrait") => env.width < env.height,
                ("prefers-color-scheme", "light") => true,
                ("prefers-color-scheme", "dark") => false,
                ("prefers-reduced-motion", _) => true,
                ("hover", "hover") => true,
                ("pointer", "fine") => true,
                ("scripting", "enabled") => env.scripting,
                ("scripting", "none") => !env.scripting,
                ("scripting", "initial-only") => false,
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

/// Integer device features (Media Queries 4 §6): bits per color component
/// (8), monochrome bits (0), color map entries (0), grid device (0).
fn device_integer(base: &str) -> Option<f32> {
    match base {
        "color" => Some(8.0),
        "monochrome" | "color-index" | "grid" => Some(0.0),
        _ => None,
    }
}

fn is_numeric_feature(name: &str) -> bool {
    let base = name.strip_prefix("min-").or_else(|| name.strip_prefix("max-")).unwrap_or(name);
    device_integer(base).is_some()
}

/// Evaluate `(color)`, `(min-color: 1)`, `(max-monochrome: 0)`, ...
fn numeric_feature(name: &str, value: &str) -> bool {
    let (op, base) = match (name.strip_prefix("min-"), name.strip_prefix("max-")) {
        (Some(b), _) => ('>', b),
        (_, Some(b)) => ('<', b),
        _ => ('=', name),
    };
    let Some(actual) = device_integer(base) else { return false };
    if value.is_empty() {
        // Boolean context; `min-`/`max-` need a value.
        return op == '=' && actual != 0.0;
    }
    match value.parse::<f32>() {
        Ok(n) if n >= 0.0 && n.fract() == 0.0 => match op {
            '>' => actual >= n,
            '<' => actual <= n,
            _ => actual == n,
        },
        _ => false,
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
                    Rule::Import { .. } | Rule::FontFace(_) | Rule::Keyframes { .. } => {}
                }
            }
        }
        walk(&self.rules, env, out);
    }

    /// All `@font-face` rules (inside active media blocks too).
    pub fn font_faces<'a>(&'a self, env: &MediaEnv, out: &mut Vec<&'a [Declaration]>) {
        fn walk<'a>(rules: &'a [Rule], env: &MediaEnv, out: &mut Vec<&'a [Declaration]>) {
            for r in rules {
                match r {
                    Rule::FontFace(d) => out.push(d),
                    Rule::Media { query, rules } if query.evaluate(env) => walk(rules, env, out),
                    Rule::Supports { matches: true, rules } => walk(rules, env, out),
                    _ => {}
                }
            }
        }
        walk(&self.rules, env, out);
    }

    /// All `@keyframes` rules as (name, frames).
    pub fn keyframes<'a>(&'a self, out: &mut Vec<(&'a str, &'a [Keyframe])>) {
        fn walk<'a>(rules: &'a [Rule], out: &mut Vec<(&'a str, &'a [Keyframe])>) {
            for r in rules {
                match r {
                    Rule::Keyframes { name, frames } => out.push((name, frames)),
                    Rule::Media { rules, .. } | Rule::Supports { rules, .. } => walk(rules, out),
                    _ => {}
                }
            }
        }
        walk(&self.rules, out);
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

thread_local! {
    static ANON_LAYERS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Layer names of an `@layer` prelude (`a.b, c`).
fn layer_names(prelude: &[Token]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for t in prelude {
        match t {
            Token::Ident(s) => cur.push_str(s),
            Token::Delim('.') => cur.push('.'),
            Token::Comma => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Put the rules of an `@layer name { }` block (and of the layers nested
/// in it) into `name`.
fn assign_layer(rules: &mut [Rule], name: &str) {
    for r in rules {
        match r {
            Rule::Style(s) => {
                s.layer = Some(match &s.layer {
                    Some(inner) => std::sync::Arc::from(format!("{name}.{inner}").as_str()),
                    None => std::sync::Arc::from(name),
                });
            }
            Rule::Media { rules, .. } | Rule::Supports { rules, .. } => assign_layer(rules, name),
            _ => {}
        }
    }
}

pub fn parse_stylesheet(src: &str) -> Stylesheet {
    // `<style>` text of XHTML documents wrapped in a CDATA section (the
    // XML parser drops the markers; our HTML parser keeps them as text).
    let stripped;
    let src = if src.contains("<![CDATA[") {
        stripped = src.replacen("<![CDATA[", "", 1).replace("]]>", "");
        stripped.as_str()
    } else {
        src
    };
    let toks = Tokenizer::tokenize(src);
    let mut p = Parser { toks: &toks, pos: 0 };
    Stylesheet {
        rules: p.parse_rules(true),
        base_url: None,
        shadow_scopes: Vec::new(),
        source_id: 0,
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
        self.parse_rule_context(top_level, None, 0)
    }

    fn parse_rule_context(&mut self, top_level: bool, parent: Option<std::sync::Arc<Vec<Selector>>>, depth: usize) -> Vec<Rule> {
        if depth > 64 { return Vec::new(); }
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
                                        rules: if let Some(p) = &parent { sub.parse_style_body(p.clone(), depth + 1) } else { sub.parse_rule_context(false, None, depth + 1) },
                                    });
                                }
                                "supports" => {
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    rules.push(Rule::Supports {
                                        matches: supports::evaluate(prelude),
                                        rules: if let Some(p) = &parent { sub.parse_style_body(p.clone(), depth + 1) } else { sub.parse_rule_context(false, None, depth + 1) },
                                    });
                                }
                                "layer" => {
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    let mut inner_rules = if let Some(p) = &parent { sub.parse_style_body(p.clone(), depth + 1) } else { sub.parse_rule_context(false, None, depth + 1) };
                                    let name = layer_names(prelude).into_iter().next().unwrap_or_else(|| {
                                        // An anonymous layer is unique.
                                        let n = ANON_LAYERS.with(|c| {
                                            c.set(c.get() + 1);
                                            c.get()
                                        });
                                        format!("#anon{n}")
                                    });
                                    // Register the layer even when it is empty.
                                    rules.push(Rule::Style(StyleRule { selectors: Vec::new(), declarations: Vec::new(), layer: Some(std::sync::Arc::from(name.as_str())) }));
                                    assign_layer(&mut inner_rules, &name);
                                    rules.extend(inner_rules);
                                }
                                "container" | "scope" => {
                                    // Transparent grouping.
                                    let mut sub = Parser { toks: inner, pos: 0 };
                                    rules.extend(if let Some(p) = &parent { sub.parse_style_body(p.clone(), depth + 1) } else { sub.parse_rule_context(false, None, depth + 1) });
                                }
                                "font-face" if parent.is_none() => {
                                    let mut dp = Parser { toks: inner, pos: 0 };
                                    rules.push(Rule::FontFace(dp.parse_declarations()));
                                }
                                "keyframes" | "-webkit-keyframes" if parent.is_none() => {
                                    let kname = prelude
                                        .iter()
                                        .find_map(|t| match t {
                                            Token::Ident(s) | Token::String(s) => Some(s.clone()),
                                            _ => None,
                                        })
                                        .unwrap_or_default();
                                    let mut kp = Parser { toks: inner, pos: 0 };
                                    rules.push(Rule::Keyframes {
                                        name: kname,
                                        frames: kp.parse_keyframes(),
                                    });
                                }
                                _ => {} // page, counter-style, property, ...
                            }
                        }
                        _ => {
                            // Statement at-rule
                            if self.peek() == Some(&Token::Semicolon) {
                                self.pos += 1;
                            }
                            if name == "import" && parent.is_none() {
                                if let Some(imp) = parse_import(prelude) {
                                    rules.push(imp);
                                }
                            }
                            if name == "layer" && parent.is_none() {
                                // `@layer a, b;` fixes the order of layers.
                                for n in layer_names(prelude) {
                                    rules.push(Rule::Style(StyleRule { selectors: Vec::new(), declarations: Vec::new(), layer: Some(std::sync::Arc::from(n.as_str())) }));
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
                    let mut sp = if let Some(p) = &parent { selector::SelectorParser::nested(prelude, p.clone()) } else { selector::SelectorParser::new(prelude) };
                    if let Some(selectors) = sp.parse_list() {
                        let mut dp = Parser { toks: inner, pos: 0 };
                        rules.extend(dp.parse_style_body(std::sync::Arc::new(selectors), depth + 1));
                    }
                }
            }
        }
        rules
    }

    /// Preserve declaration runs around nested rules in source order. Parent
    /// selector lists are shared by the nesting AST rather than expanded.
    fn parse_style_body(&mut self, parent: std::sync::Arc<Vec<Selector>>, depth: usize) -> Vec<Rule> {
        if depth > 64 { return Vec::new(); }
        let mut budget = 4096;
        if !parent.iter().all(|s| s.fits_nesting_budget(&mut budget)) { return Vec::new(); }
        let mut rules = Vec::new();
        let mut declarations = Vec::new();
        loop {
            self.skip_ws();
            if self.peek().is_none() { break; }
            if self.peek() == Some(&Token::Semicolon) { self.pos += 1; continue; }
            let start = self.pos;
            let at_rule = matches!(self.peek(), Some(Token::AtKeyword(_)));
            let custom = matches!(self.peek(), Some(Token::Ident(n)) if n.starts_with("--"))
                && self.toks[self.pos + 1..].iter().find(|t| **t != Token::Whitespace) == Some(&Token::Colon);
            self.consume_until(true, !custom);
            let block = self.peek() == Some(&Token::OpenBrace);
            if block { self.consume_block(); }
            if self.peek() == Some(&Token::Semicolon) { self.pos += 1; }
            let end = self.pos;
            if end == start { self.pos += 1; continue; }
            let mut sub = Parser { toks: &self.toks[start..end], pos: 0 };
            if at_rule || block {
                if !declarations.is_empty() {
                    rules.push(Rule::Style(StyleRule { selectors: parent.as_ref().clone(), declarations: std::mem::take(&mut declarations), layer: None }));
                }
                rules.extend(sub.parse_rule_context(false, Some(parent.clone()), depth + 1));
            } else {
                declarations.extend(sub.parse_declarations());
            }
        }
        if !declarations.is_empty() {
            rules.push(Rule::Style(StyleRule { selectors: parent.as_ref().clone(), declarations, layer: None }));
        }
        rules
    }

    /// Parse the body of `@keyframes`: `selector-list { declarations }` blocks.
    fn parse_keyframes(&mut self) -> Vec<Keyframe> {
        let mut frames = Vec::new();
        loop {
            self.skip_ws();
            if self.peek().is_none() {
                break;
            }
            let prelude = self.consume_until(false, true);
            if self.peek() != Some(&Token::OpenBrace) {
                break;
            }
            let inner = self.consume_block();
            let mut offsets = Vec::new();
            for t in prelude {
                match t {
                    Token::Ident(i) if i.eq_ignore_ascii_case("from") => offsets.push(0.0),
                    Token::Ident(i) if i.eq_ignore_ascii_case("to") => offsets.push(1.0),
                    Token::Percentage(p) => offsets.push((p / 100.0).clamp(0.0, 1.0)),
                    _ => {}
                }
            }
            let mut dp = Parser { toks: inner, pos: 0 };
            let declarations = dp.parse_declarations();
            if !offsets.is_empty() {
                frames.push(Keyframe { offsets, declarations });
            }
        }
        frames
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
                    // Custom property names are case-sensitive
                    // (`--xhUGwc` and `--xhugwc` are different).
                    let name = if name.starts_with("--") { name.clone() } else { name.to_ascii_lowercase() };
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
                    // `!important` (CSS Syntax 3 §5.4.6: the last two
                    // non-whitespace tokens are `!` and `important`).
                    let mut important = false;
                    if let Some(Token::Ident(i)) = value.last() {
                        if i.eq_ignore_ascii_case("important") {
                            let mut k = value.len() - 1;
                            while k > 0 && value[k - 1] == Token::Whitespace {
                                k -= 1;
                            }
                            if k > 0 && value[k - 1] == Token::Delim('!') {
                                important = true;
                                value.truncate(k - 1);
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
    fn nesting_parent_lists_order_and_conditions() {
        let sheet = parse_stylesheet(".parent, #absent { color:red; > span { color:blue } color:green; @media (min-width:1px) { @supports(display:grid) { &.parent { color:black } } } }");
        let env = MediaEnv { width: 100., height: 100., screen: true, scripting: true };
        let mut rules = Vec::new();
        sheet.active_rules(&env, &mut rules);
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].declarations[0].value, vec![Token::Ident("red".into())]);
        assert_eq!(rules[2].declarations[0].value, vec![Token::Ident("green".into())]);
        assert_eq!(rules[1].selectors[0].specificity(), Specificity(1, 0, 1));
        assert_eq!(rules[3].selectors[0].specificity(), Specificity(1, 1, 0));
        let doc = rabbithole_html::parse("<div class=parent><span></span></div><span></span>");
        assert!(matches(&rules[1].selectors[0], &doc, doc.find_element("span").unwrap()));
        assert!(matches(&rules[3].selectors[0], &doc, doc.find_element("div").unwrap()));
    }

    #[test]
    fn nesting_explicit_ampersand_inside_functions_suppresses_implicit_parent() {
        let sheet = parse_stylesheet(".outer { :is(.target, &.absent) {color:green} :where(&) > span {color:blue} } ");
        let env = MediaEnv { width: 100., height: 100., screen: true, scripting: true };
        let mut rules = Vec::new(); sheet.active_rules(&env, &mut rules);
        let doc = rabbithole_html::parse("<div class=target></div><section class=outer><span></span></section>");
        assert_eq!(rules.len(), 2);
        assert!(matches(&rules[0].selectors[0], &doc, doc.find_element("div").unwrap()));
        assert!(matches(&rules[1].selectors[0], &doc, doc.find_element("span").unwrap()));
        assert_eq!(rules[1].selectors[0].specificity(), Specificity(0, 0, 1));
    }

    #[test]
    fn nesting_custom_braces_invalid_rules_and_pseudo_elements() {
        let sheet = parse_stylesheet("div::before {content:'x'; & {color:red} color:green} .x {--a:{x:y}; &foo {color:red} & {color:blue} }");
        let env = MediaEnv { width: 100., height: 100., screen: true, scripting: true };
        let mut rules = Vec::new(); sheet.active_rules(&env, &mut rules);
        assert_eq!(rules.len(), 5);
        assert_eq!(rules[0].selectors[0].pseudo_element, Some(PseudoElement::Before));
        assert_eq!(rules[2].selectors[0].pseudo_element, Some(PseudoElement::Before));
        assert_eq!(rules[3].declarations[0].name, "--a");
        let doc = rabbithole_html::parse("<div class=x></div>");
        let div = doc.find_element("div").unwrap();
        assert!(!matches(&rules[1].selectors[0], &doc, div));
        assert!(matches(&rules[4].selectors[0], &doc, div));
    }

    #[test]
    fn nesting_repeated_parent_work_is_bounded_and_following_rules_survive() {
        let mut css = String::from(".x {");
        for _ in 0..20 { css.push_str("&& {"); }
        css.push_str("color:red;");
        for _ in 0..21 { css.push('}'); }
        css.push_str(".safe { color:green }");
        let sheet = parse_stylesheet(&css);
        let env = MediaEnv { width: 100., height: 100., screen: true, scripting: true };
        let mut rules = Vec::new(); sheet.active_rules(&env, &mut rules);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].selectors[0].compounds[0].classes, vec!["safe"]);
    }

    #[test]
    fn media_queries_color_features() {
        let env = MediaEnv { width: 0.0, height: 0.0, screen: true, scripting: true };
        let m = |q: &str| parse_media_query(q).evaluate(&env);
        assert!(m("all and (min-color: 0)"));
        assert!(m("only all and (min-color: 0)"));
        assert!(!m("not all and (min-color: 0)"));
        assert!(!m("(bogus)"));
        assert!(m("(bogus), all"));
        assert!(!m("(max-color: 0) and (max-monochrome: 0)"));
        assert!(m("(min-color: 1), (min-monochrome: 1)"));
        assert!(m("all and (min-color: 0) and (min-monochrome: 0)"));
        assert!(m("not all and (min-color: 1), not all and (min-monochrome: 1)"));
        assert!(m("(color)") && !m("(monochrome)"));
        assert!(!m("all and color") && !m("all and min-color: 0"));
        assert!(m("all and (max-height: 1em) and (max-width: 1em)"));
    }

    #[test]
    fn parse_sheet() {
        let css = "@import url(a.css) screen; @charset 'utf-8'; body { color: red; margin: 0 !important } \
                   @media (max-width: 600px) { .x { display: none } } p:hover, p { font: 12px/1.5 serif } \
                   @font-face { font-family: X; src: url(x.woff) } bad { ; } a { }";
        let sheet = parse_stylesheet(css);
        assert_eq!(sheet.imports().len(), 1);
        let env = MediaEnv { width: 500.0, height: 800.0, screen: true, scripting: true };
        let mut rules = Vec::new();
        sheet.active_rules(&env, &mut rules);
        assert_eq!(rules.len(), 3, "{:?}", rules);
        assert!(rules[0].declarations[1].important);
        let env2 = MediaEnv { width: 900.0, height: 800.0, screen: true, scripting: true };
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
        let env = MediaEnv { width: 700.0, height: 800.0, screen: true, scripting: true };
        assert!(q.evaluate(&env));
        let env = MediaEnv { width: 1000.0, height: 800.0, screen: true, scripting: true };
        assert!(!q.evaluate(&env));
        let q = parse_media_query("not screen");
        assert!(!q.evaluate(&env));
        let q = parse_media_query("(width >= 600px)");
        assert!(q.evaluate(&env));
    }
}
