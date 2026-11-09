//! HTML tokenizer: a pragmatic implementation of the WHATWG tokenizer
//! state machine covering everything real pages use (tags, attributes,
//! comments, doctype, CDATA, raw text elements and character references).

use super::entities;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Doctype(String),
    StartTag {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    /// Text; `raw` means character references were not decoded (RAWTEXT /
    /// script data) and whitespace handling is the caller's business.
    Text(String),
    Comment(String),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKind {
    /// `<script>`, `<style>`, `<xmp>`, `<iframe>`, `<noembed>`, `<noframes>`
    RawText,
    /// `<textarea>`, `<title>` (entities decoded)
    RcData,
    /// `<plaintext>`: the rest of the document
    PlainText,
}

pub struct Tokenizer<'a> {
    input: &'a str,
    pos: usize,
    /// When set, the next text token is read as raw text until `</name`.
    raw: Option<(RawKind, String)>,
    done: bool,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str) -> Self {
        Tokenizer {
            input,
            pos: 0,
            raw: None,
            done: false,
        }
    }

    /// Tell the tokenizer the just-emitted start tag switched the content
    /// model (the tree builder knows which elements are raw text).
    pub fn set_raw(&mut self, kind: RawKind, name: &str) {
        self.raw = Some((kind, name.to_ascii_lowercase()));
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.input.as_bytes().get(self.pos + off).copied()
    }

    fn starts_with_ci(&self, s: &str) -> bool {
        let end = self.pos + s.len();
        end <= self.input.len() && self.input.as_bytes()[self.pos..end].eq_ignore_ascii_case(s.as_bytes())
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    pub fn next_token(&mut self) -> Token {
        if self.done {
            return Token::Eof;
        }
        if let Some((kind, name)) = self.raw.take() {
            return self.raw_text(kind, &name);
        }
        if self.pos >= self.input.len() {
            self.done = true;
            return Token::Eof;
        }
        if self.peek() == Some(b'<') {
            if let Some(tok) = self.tag_open() {
                return tok;
            }
        }
        // Text run up to the next '<' that plausibly starts markup.
        let start = self.pos;
        self.pos += 1; // consume at least one char (may be a stray '<')
        while self.pos < self.input.len() {
            if self.input.as_bytes()[self.pos] == b'<' {
                let next = self.peek_at(1);
                if matches!(next, Some(c) if c.is_ascii_alphabetic() || c == b'/' || c == b'!' || c == b'?') {
                    break;
                }
            }
            self.pos += 1;
        }
        let text = &self.input[start..self.pos];
        Token::Text(entities::decode(text, false).replace('\0', "\u{fffd}"))
    }

    fn raw_text(&mut self, kind: RawKind, name: &str) -> Token {
        let start = self.pos;
        if kind == RawKind::PlainText {
            self.pos = self.input.len();
            return Token::Text(self.input[start..].to_string());
        }
        // Find "</name" followed by whitespace, '/', or '>'
        let bytes = self.input.as_bytes();
        let mut i = start;
        let end;
        loop {
            match self.input[i..].find("</") {
                None => {
                    end = self.input.len();
                    break;
                }
                Some(off) => {
                    let p = i + off;
                    let after = p + 2;
                    if after + name.len() <= bytes.len()
                        && bytes[after..after + name.len()].eq_ignore_ascii_case(name.as_bytes())
                    {
                        let follow = bytes.get(after + name.len()).copied();
                        if matches!(follow, None | Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'\x0c'))
                        {
                            end = p;
                            break;
                        }
                    }
                    i = p + 2;
                }
            }
        }
        self.pos = end;
        let text = &self.input[start..end];
        let text = match kind {
            RawKind::RcData => entities::decode(text, false),
            _ => text.to_string(),
        };
        Token::Text(text)
    }

    /// Called at '<'. Returns None if this is not actually markup.
    fn tag_open(&mut self) -> Option<Token> {
        let c = self.peek_at(1)?;
        if c == b'!' {
            self.pos += 2;
            return Some(self.markup_declaration());
        }
        if c == b'?' {
            // Bogus comment
            self.pos += 1;
            return Some(self.bogus_comment());
        }
        if c == b'/' {
            let c2 = self.peek_at(2);
            match c2 {
                Some(c2) if c2.is_ascii_alphabetic() => {
                    self.pos += 2;
                    let name = self.tag_name();
                    // Skip attributes on end tags
                    self.skip_to_tag_end();
                    return Some(Token::EndTag { name });
                }
                Some(b'>') => {
                    // "</>" is ignored entirely
                    self.pos += 3;
                    return Some(self.next_token());
                }
                None => {
                    self.pos = self.input.len();
                    return Some(Token::Eof);
                }
                _ => {
                    self.pos += 2;
                    return Some(self.bogus_comment());
                }
            }
        }
        if c.is_ascii_alphabetic() {
            self.pos += 1;
            let name = self.tag_name();
            let (attrs, self_closing) = self.attributes();
            return Some(Token::StartTag {
                name,
                attrs,
                self_closing,
            });
        }
        None
    }

    fn tag_name(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || c == b'/' || c == b'>' {
                break;
            }
            self.pos += 1;
        }
        self.input[start..self.pos].to_ascii_lowercase()
    }

    fn skip_to_tag_end(&mut self) {
        // Consume (ignored) attribute-ish content until '>'
        let (_, _) = self.attributes();
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn attributes(&mut self) -> (Vec<(String, String)>, bool) {
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') => {
                    self.pos += 1;
                    if self.peek() == Some(b'>') {
                        self.pos += 1;
                        self_closing = true;
                        break;
                    }
                    continue;
                }
                Some(_) => {}
            }
            // Attribute name
            let start = self.pos;
            // A leading '=' or quote is part of the name per spec; just consume one.
            if matches!(self.peek(), Some(b'=') | Some(b'"') | Some(b'\'')) {
                self.pos += 1;
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_whitespace() || c == b'/' || c == b'>' || c == b'=' {
                    break;
                }
                self.pos += 1;
            }
            let name = self.input[start..self.pos].to_ascii_lowercase();
            self.skip_ws();
            let mut value = String::new();
            if self.peek() == Some(b'=') {
                self.pos += 1;
                self.skip_ws();
                match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.pos += 1;
                        let vstart = self.pos;
                        while let Some(c) = self.peek() {
                            if c == q {
                                break;
                            }
                            self.pos += 1;
                        }
                        value = entities::decode(&self.input[vstart..self.pos], true);
                        if self.peek() == Some(q) {
                            self.pos += 1;
                        }
                    }
                    Some(b'>') => {}
                    Some(_) => {
                        let vstart = self.pos;
                        while let Some(c) = self.peek() {
                            if c.is_ascii_whitespace() || c == b'>' {
                                break;
                            }
                            self.pos += 1;
                        }
                        value = entities::decode(&self.input[vstart..self.pos], true);
                    }
                    None => {}
                }
            }
            if !name.is_empty() && !attrs.iter().any(|(n, _)| *n == name) {
                attrs.push((name, value));
            }
        }
        (attrs, self_closing)
    }

    fn markup_declaration(&mut self) -> Token {
        if self.starts_with("--") {
            self.pos += 2;
            // Comment: ends at "-->" ; also handle "--!>" and abrupt "-->" right away
            let start = self.pos;
            if self.starts_with(">") {
                self.pos += 1;
                return Token::Comment(String::new());
            }
            if self.starts_with("->") {
                self.pos += 2;
                return Token::Comment(String::new());
            }
            let rest = &self.input[start..];
            let end = rest.find("-->").map(|e| (e, 3));
            let end2 = rest.find("--!>").map(|e| (e, 4));
            let chosen = match (end, end2) {
                (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
                (a, b) => a.or(b),
            };
            match chosen {
                Some((e, len)) => {
                    let text = rest[..e].to_string();
                    self.pos = start + e + len;
                    Token::Comment(text)
                }
                None => {
                    let text = rest.to_string();
                    self.pos = self.input.len();
                    Token::Comment(text)
                }
            }
        } else if self.starts_with_ci("doctype") {
            self.pos += 7;
            let start = self.pos;
            while let Some(c) = self.peek() {
                if c == b'>' {
                    break;
                }
                self.pos += 1;
            }
            let text = self.input[start..self.pos].trim().to_string();
            if self.peek() == Some(b'>') {
                self.pos += 1;
            }
            Token::Doctype(text)
        } else if self.starts_with("[CDATA[") {
            self.pos += 7;
            let start = self.pos;
            let rest = &self.input[start..];
            match rest.find("]]>") {
                Some(e) => {
                    let text = rest[..e].to_string();
                    self.pos = start + e + 3;
                    // Outside foreign content CDATA is a bogus comment, but
                    // treating it as text is more useful for our purposes.
                    Token::Comment(text)
                }
                None => {
                    self.pos = self.input.len();
                    Token::Comment(rest.to_string())
                }
            }
        } else {
            self.bogus_comment()
        }
    }

    fn bogus_comment(&mut self) -> Token {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == b'>' {
                break;
            }
            self.pos += 1;
        }
        let text = self.input[start..self.pos].to_string();
        if self.peek() == Some(b'>') {
            self.pos += 1;
        }
        Token::Comment(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &str) -> Vec<Token> {
        let mut t = Tokenizer::new(input);
        let mut v = Vec::new();
        loop {
            let tok = t.next_token();
            if tok == Token::Eof {
                break;
            }
            if let Token::StartTag { name, .. } = &tok {
                if name == "script" || name == "style" {
                    t.set_raw(RawKind::RawText, name);
                }
            }
            v.push(tok);
        }
        v
    }

    #[test]
    fn basic() {
        let toks = all("<!DOCTYPE html><p class=\"a b\" id=x hidden>Hi &amp; <b>bye</b></p><!-- c --><br/>");
        assert_eq!(toks[0], Token::Doctype("html".into()));
        assert_eq!(
            toks[1],
            Token::StartTag {
                name: "p".into(),
                attrs: vec![("class".into(), "a b".into()), ("id".into(), "x".into()), ("hidden".into(), String::new())],
                self_closing: false
            }
        );
        assert_eq!(toks[2], Token::Text("Hi & ".into()));
        assert_eq!(toks[3], Token::StartTag { name: "b".into(), attrs: vec![], self_closing: false });
        assert_eq!(toks[4], Token::Text("bye".into()));
        assert_eq!(toks[5], Token::EndTag { name: "b".into() });
        assert_eq!(toks[6], Token::EndTag { name: "p".into() });
        assert_eq!(toks[7], Token::Comment(" c ".into()));
        assert_eq!(toks[8], Token::StartTag { name: "br".into(), attrs: vec![], self_closing: true });
    }

    #[test]
    fn script_raw() {
        let toks = all("<script>if (a < b && c) { x = '</div>'; }</script><p>x</p>");
        assert_eq!(toks[1], Token::Text("if (a < b && c) { x = '</div>'; }".into()));
        assert_eq!(toks[2], Token::EndTag { name: "script".into() });
    }

    #[test]
    fn stray_lt() {
        let toks = all("a < b <p>");
        assert_eq!(toks[0], Token::Text("a < b ".into()));
    }
}
