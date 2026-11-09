//! CSS Syntax Level 3 tokenizer.

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Function(String),
    AtKeyword(String),
    Hash(String),
    String(String),
    Url(String),
    Number(f32),
    Percentage(f32),
    Dimension(f32, String),
    Whitespace,
    Colon,
    Semicolon,
    Comma,
    OpenBracket,
    CloseBracket,
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    Delim(char),
    BadString,
    BadUrl,
    Cdo,
    Cdc,
}

pub struct Tokenizer<'a> {
    input: &'a [u8],
    src: &'a str,
    pos: usize,
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
fn is_name(c: u8) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == b'-'
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a str) -> Self {
        Tokenizer {
            input: src.as_bytes(),
            src,
            pos: 0,
        }
    }

    pub fn tokenize(src: &'a str) -> Vec<Token> {
        let mut t = Tokenizer::new(src);
        let mut out = Vec::new();
        while let Some(tok) = t.next_token() {
            out.push(tok);
        }
        out
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.input.get(self.pos + off).copied()
    }

    fn skip_comments_and_ws(&mut self) -> bool {
        let mut saw_ws = false;
        loop {
            match self.peek(0) {
                Some(c) if c.is_ascii_whitespace() => {
                    saw_ws = true;
                    self.pos += 1;
                }
                Some(b'/') if self.peek(1) == Some(b'*') => {
                    self.pos += 2;
                    match self.src[self.pos..].find("*/") {
                        Some(e) => self.pos += e + 2,
                        None => self.pos = self.input.len(),
                    }
                }
                _ => return saw_ws,
            }
        }
    }

    fn would_start_number(&self, off: usize) -> bool {
        match self.peek(off) {
            Some(c) if c.is_ascii_digit() => true,
            Some(b'.') => matches!(self.peek(off + 1), Some(c) if c.is_ascii_digit()),
            Some(b'+') | Some(b'-') => match self.peek(off + 1) {
                Some(c) if c.is_ascii_digit() => true,
                Some(b'.') => matches!(self.peek(off + 2), Some(c) if c.is_ascii_digit()),
                _ => false,
            },
            _ => false,
        }
    }

    fn would_start_ident(&self, off: usize) -> bool {
        match self.peek(off) {
            Some(b'-') => match self.peek(off + 1) {
                Some(b'-') => true,
                Some(c) if is_name_start(c) => true,
                Some(b'\\') => true,
                _ => false,
            },
            Some(c) if is_name_start(c) => true,
            Some(b'\\') => !matches!(self.peek(off + 1), Some(b'\n') | None),
            _ => false,
        }
    }

    fn consume_escape(&mut self) -> char {
        // Assumes the backslash has been consumed.
        match self.peek(0) {
            Some(c) if c.is_ascii_hexdigit() => {
                let start = self.pos;
                while self.pos - start < 6 && matches!(self.peek(0), Some(c) if c.is_ascii_hexdigit()) {
                    self.pos += 1;
                }
                let cp = u32::from_str_radix(&self.src[start..self.pos], 16).unwrap_or(0xfffd);
                if matches!(self.peek(0), Some(c) if c.is_ascii_whitespace()) {
                    self.pos += 1;
                }
                if cp == 0 || cp > 0x10ffff || (0xd800..=0xdfff).contains(&cp) {
                    '\u{fffd}'
                } else {
                    char::from_u32(cp).unwrap_or('\u{fffd}')
                }
            }
            Some(_) => {
                let ch = self.src[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
                ch
            }
            None => '\u{fffd}',
        }
    }

    fn consume_name(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.peek(0) {
                Some(c) if is_name(c) => {
                    if c < 0x80 {
                        out.push(c as char);
                        self.pos += 1;
                    } else {
                        let ch = self.src[self.pos..].chars().next().unwrap();
                        out.push(ch);
                        self.pos += ch.len_utf8();
                    }
                }
                Some(b'\\') if !matches!(self.peek(1), Some(b'\n') | None) => {
                    self.pos += 1;
                    out.push(self.consume_escape());
                }
                _ => return out,
            }
        }
    }

    fn consume_number(&mut self) -> f32 {
        let start = self.pos;
        if matches!(self.peek(0), Some(b'+') | Some(b'-')) {
            self.pos += 1;
        }
        while matches!(self.peek(0), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek(0) == Some(b'.') && matches!(self.peek(1), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
            while matches!(self.peek(0), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(0), Some(b'e') | Some(b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(0), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if matches!(self.peek(0), Some(c) if c.is_ascii_digit()) {
                while matches!(self.peek(0), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }
        self.src[start..self.pos].parse().unwrap_or(0.0)
    }

    fn consume_string(&mut self, quote: u8) -> Token {
        let mut out = String::new();
        loop {
            match self.peek(0) {
                None => return Token::String(out),
                Some(c) if c == quote => {
                    self.pos += 1;
                    return Token::String(out);
                }
                Some(b'\n') => return Token::BadString,
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek(0) {
                        None => {}
                        Some(b'\n') => self.pos += 1,
                        Some(_) => out.push(self.consume_escape()),
                    }
                }
                Some(_) => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn consume_url(&mut self) -> Token {
        // After "url(" ; whitespace skipped by caller
        let mut out = String::new();
        loop {
            match self.peek(0) {
                None => return Token::Url(out),
                Some(b')') => {
                    self.pos += 1;
                    return Token::Url(out);
                }
                Some(c) if c.is_ascii_whitespace() => {
                    self.skip_comments_and_ws();
                    if self.peek(0) == Some(b')') {
                        self.pos += 1;
                    }
                    return Token::Url(out);
                }
                Some(b'"') | Some(b'\'') | Some(b'(') => {
                    // bad url: consume to ')'
                    while let Some(c) = self.peek(0) {
                        self.pos += 1;
                        if c == b')' {
                            break;
                        }
                    }
                    return Token::BadUrl;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    out.push(self.consume_escape());
                }
                Some(_) => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    pub fn next_token(&mut self) -> Option<Token> {
        if self.skip_comments_and_ws() {
            return Some(Token::Whitespace);
        }
        let c = self.peek(0)?;
        let tok = match c {
            b'"' | b'\'' => {
                self.pos += 1;
                self.consume_string(c)
            }
            b'#' => {
                if matches!(self.peek(1), Some(n) if is_name(n)) || self.peek(1) == Some(b'\\') {
                    self.pos += 1;
                    Token::Hash(self.consume_name())
                } else {
                    self.pos += 1;
                    Token::Delim('#')
                }
            }
            b'(' => {
                self.pos += 1;
                Token::OpenParen
            }
            b')' => {
                self.pos += 1;
                Token::CloseParen
            }
            b'[' => {
                self.pos += 1;
                Token::OpenBracket
            }
            b']' => {
                self.pos += 1;
                Token::CloseBracket
            }
            b'{' => {
                self.pos += 1;
                Token::OpenBrace
            }
            b'}' => {
                self.pos += 1;
                Token::CloseBrace
            }
            b',' => {
                self.pos += 1;
                Token::Comma
            }
            b':' => {
                self.pos += 1;
                Token::Colon
            }
            b';' => {
                self.pos += 1;
                Token::Semicolon
            }
            b'<' if self.src[self.pos..].starts_with("<!--") => {
                self.pos += 4;
                Token::Cdo
            }
            b'-' if self.src[self.pos..].starts_with("-->") => {
                self.pos += 3;
                Token::Cdc
            }
            b'@' => {
                if self.would_start_ident(1) {
                    self.pos += 1;
                    Token::AtKeyword(self.consume_name())
                } else {
                    self.pos += 1;
                    Token::Delim('@')
                }
            }
            _ if self.would_start_number(0) => {
                let n = self.consume_number();
                if self.peek(0) == Some(b'%') {
                    self.pos += 1;
                    Token::Percentage(n)
                } else if self.would_start_ident(0) {
                    let unit = self.consume_name().to_ascii_lowercase();
                    Token::Dimension(n, unit)
                } else {
                    Token::Number(n)
                }
            }
            _ if self.would_start_ident(0) => {
                let name = self.consume_name();
                if self.peek(0) == Some(b'(') {
                    self.pos += 1;
                    if name.eq_ignore_ascii_case("url") {
                        self.skip_comments_and_ws();
                        if matches!(self.peek(0), Some(b'"') | Some(b'\'')) {
                            Token::Function(name)
                        } else {
                            self.consume_url()
                        }
                    } else {
                        Token::Function(name)
                    }
                } else {
                    Token::Ident(name)
                }
            }
            _ => {
                let ch = self.src[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
                Token::Delim(ch)
            }
        };
        Some(tok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        let toks = Tokenizer::tokenize("a.b#c > d { color: #fff; width: 10px; x: url(foo.png) 1.5em -2% }");
        assert!(toks.contains(&Token::Hash("c".into())));
        assert!(toks.contains(&Token::Dimension(10.0, "px".into())));
        assert!(toks.contains(&Token::Url("foo.png".into())));
        assert!(toks.contains(&Token::Dimension(1.5, "em".into())));
        assert!(toks.contains(&Token::Percentage(-2.0)));
        assert!(toks.contains(&Token::Delim('>')));
    }
}
