//! The HTML tokenizer (WHATWG HTML §13.2.5): the full state machine,
//! including script data escaping, CDATA sections (when the tree builder
//! allows them), character references with the longest-match rule and
//! DOCTYPE identifiers.

use std::collections::VecDeque;

use crate::entities;

#[derive(Debug, Clone, PartialEq)]
pub struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub force_quirks: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Doctype(Doctype),
    StartTag { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    EndTag { name: String },
    Comment(String),
    /// A run of character tokens.
    Text(String),
    Eof,
}

/// Tokenizer states the tree builder switches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
    TagOpen,
    EndTagOpen,
    TagName,
    RcdataLessThan,
    RcdataEndTagOpen,
    RcdataEndTagName,
    RawtextLessThan,
    RawtextEndTagOpen,
    RawtextEndTagName,
    ScriptDataLessThan,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThan,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThan,
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThan,
    CommentLessThanBang,
    CommentLessThanBangDash,
    CommentLessThanBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicId,
    DoctypePublicIdDoubleQuoted,
    DoctypePublicIdSingleQuoted,
    AfterDoctypePublicId,
    BetweenDoctypePublicAndSystem,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemId,
    DoctypeSystemIdDoubleQuoted,
    DoctypeSystemIdSingleQuoted,
    AfterDoctypeSystemId,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
    CharacterReference,
    NamedCharacterReference,
    AmbiguousAmpersand,
    NumericCharacterReference,
    HexadecimalCharacterReferenceStart,
    DecimalCharacterReferenceStart,
    HexadecimalCharacterReference,
    DecimalCharacterReference,
    NumericCharacterReferenceEnd,
}

/// Legacy alias kept for callers that name raw-text kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKind {
    RawText,
    Rcdata,
    ScriptData,
    Plaintext,
}

impl RawKind {
    pub fn state(self) -> State {
        match self {
            RawKind::RawText => State::Rawtext,
            RawKind::Rcdata => State::Rcdata,
            RawKind::ScriptData => State::ScriptData,
            RawKind::Plaintext => State::Plaintext,
        }
    }
}

fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{c}' | ' ')
}

pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
    pub state: State,
    return_state: State,
    queue: VecDeque<Token>,
    text: String,
    // Current tag token.
    tag_name: String,
    tag_is_end: bool,
    tag_self_closing: bool,
    attrs: Vec<(String, String)>,
    attr_name: String,
    attr_value: String,
    in_attr: bool,
    // Current comment / doctype.
    comment: String,
    doctype: Doctype,
    temp: String,
    char_ref_code: u32,
    last_start_tag: String,
    /// Set by the tree builder: the adjusted current node is in a foreign
    /// namespace, so `<![CDATA[` opens a CDATA section.
    pub allow_cdata: bool,
    eof_emitted: bool,
}

impl Tokenizer {
    /// `input` is preprocessed: CR LF and lone CR become LF.
    pub fn new(input: &str) -> Tokenizer {
        let mut chars = Vec::with_capacity(input.len());
        let mut it = input.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\r' {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                chars.push('\n');
            } else {
                chars.push(c);
            }
        }
        Tokenizer {
            input: chars,
            pos: 0,
            state: State::Data,
            return_state: State::Data,
            queue: VecDeque::new(),
            text: String::new(),
            tag_name: String::new(),
            tag_is_end: false,
            tag_self_closing: false,
            attrs: Vec::new(),
            attr_name: String::new(),
            attr_value: String::new(),
            in_attr: false,
            comment: String::new(),
            doctype: Doctype { name: None, public_id: None, system_id: None, force_quirks: false },
            temp: String::new(),
            char_ref_code: 0,
            last_start_tag: String::new(),
            allow_cdata: false,
            eof_emitted: false,
        }
    }

    /// Switch to a raw-text state after a start tag (`<script>`, ...).
    pub fn set_raw(&mut self, kind: RawKind, name: &str) {
        self.state = kind.state();
        self.last_start_tag = name.to_ascii_lowercase();
    }

    pub fn set_state(&mut self, s: State) {
        self.state = s;
    }

    /// For fragment parsing: the context element's name is the "last start
    /// tag" for appropriate end tags.
    pub fn set_last_start_tag(&mut self, name: &str) {
        self.last_start_tag = name.to_string();
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.input.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn reconsume(&mut self, s: State) {
        self.pos -= 1;
        self.state = s;
    }

    fn lookahead(&self, s: &str, ci: bool) -> bool {
        let mut i = self.pos;
        for c in s.chars() {
            match self.input.get(i) {
                Some(&x) if x == c || (ci && x.to_ascii_lowercase() == c.to_ascii_lowercase()) => {}
                _ => return false,
            }
            i += 1;
        }
        true
    }

    fn emit_char(&mut self, c: char) {
        self.text.push(c);
    }

    fn emit_str(&mut self, s: &str) {
        self.text.push_str(s);
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.queue.push_back(Token::Text(std::mem::take(&mut self.text)));
        }
    }

    fn emit(&mut self, t: Token) {
        self.flush_text();
        self.queue.push_back(t);
    }

    fn new_tag(&mut self, end: bool) {
        self.tag_name.clear();
        self.tag_is_end = end;
        self.tag_self_closing = false;
        self.attrs.clear();
        self.attr_name.clear();
        self.attr_value.clear();
        self.in_attr = false;
    }

    fn start_attr(&mut self) {
        self.finish_attr();
        self.in_attr = true;
    }

    fn finish_attr(&mut self) {
        if self.in_attr {
            let name = std::mem::take(&mut self.attr_name);
            let value = std::mem::take(&mut self.attr_value);
            // Duplicate attributes are dropped (parse error).
            if !self.attrs.iter().any(|(n, _)| *n == name) {
                self.attrs.push((name, value));
            }
            self.in_attr = false;
        }
    }

    fn emit_tag(&mut self) {
        self.finish_attr();
        let name = std::mem::take(&mut self.tag_name);
        if self.tag_is_end {
            self.emit(Token::EndTag { name });
        } else {
            self.last_start_tag = name.clone();
            let attrs = std::mem::take(&mut self.attrs);
            self.emit(Token::StartTag { name, attrs, self_closing: self.tag_self_closing });
        }
    }

    fn appropriate_end_tag(&self) -> bool {
        !self.last_start_tag.is_empty() && self.tag_name == self.last_start_tag
    }

    fn emit_comment(&mut self) {
        let c = std::mem::take(&mut self.comment);
        self.emit(Token::Comment(c));
    }

    fn emit_doctype(&mut self) {
        let d = std::mem::replace(&mut self.doctype, Doctype { name: None, public_id: None, system_id: None, force_quirks: false });
        self.emit(Token::Doctype(d));
    }

    fn charref_in_attr(&self) -> bool {
        matches!(self.return_state, State::AttributeValueDoubleQuoted | State::AttributeValueSingleQuoted | State::AttributeValueUnquoted)
    }

    fn flush_temp(&mut self) {
        let t = std::mem::take(&mut self.temp);
        if self.charref_in_attr() {
            self.attr_value.push_str(&t);
        } else {
            self.emit_str(&t);
        }
    }

    /// The next token. After a start tag the caller may switch the state
    /// (raw text) before asking for more.
    pub fn next_token(&mut self) -> Token {
        loop {
            if let Some(t) = self.queue.pop_front() {
                return t;
            }
            if self.eof_emitted {
                return Token::Eof;
            }
            self.step();
            // Stop at a tag boundary so the tree builder can react before
            // the tokenizer moves on; keep character runs together.
            if !self.queue.is_empty() {
                continue;
            }
        }
    }

    fn eof(&mut self) {
        self.flush_text();
        self.queue.push_back(Token::Eof);
        self.eof_emitted = true;
    }

    fn step(&mut self) {
        let c = self.next_char();
        match self.state {
            State::Data => match c {
                Some('&') => {
                    self.return_state = State::Data;
                    self.state = State::CharacterReference;
                }
                Some('<') => self.state = State::TagOpen,
                Some(ch) => {
                    self.emit_char(ch);
                    // Copy the rest of a plain run at once.
                    while let Some(&n) = self.input.get(self.pos) {
                        if n == '&' || n == '<' {
                            break;
                        }
                        self.text.push(n);
                        self.pos += 1;
                    }
                }
                None => self.eof(),
            },
            State::Rcdata => match c {
                Some('&') => {
                    self.return_state = State::Rcdata;
                    self.state = State::CharacterReference;
                }
                Some('<') => self.state = State::RcdataLessThan,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::Rawtext => match c {
                Some('<') => self.state = State::RawtextLessThan,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::ScriptData => match c {
                Some('<') => self.state = State::ScriptDataLessThan,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::Plaintext => match c {
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::TagOpen => match c {
                Some('!') => self.state = State::MarkupDeclarationOpen,
                Some('/') => self.state = State::EndTagOpen,
                Some(ch) if ch.is_ascii_alphabetic() => {
                    self.new_tag(false);
                    self.reconsume(State::TagName);
                }
                Some('?') => {
                    self.comment.clear();
                    self.reconsume(State::BogusComment);
                }
                None => {
                    self.emit_char('<');
                    self.eof();
                }
                Some(_) => {
                    self.emit_char('<');
                    self.reconsume(State::Data);
                }
            },
            State::EndTagOpen => match c {
                Some(ch) if ch.is_ascii_alphabetic() => {
                    self.new_tag(true);
                    self.reconsume(State::TagName);
                }
                Some('>') => self.state = State::Data,
                None => {
                    self.emit_str("</");
                    self.eof();
                }
                Some(_) => {
                    self.comment.clear();
                    self.reconsume(State::BogusComment);
                }
            },
            State::TagName => match c {
                Some(ch) if is_ws(ch) => self.state = State::BeforeAttributeName,
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                Some('\0') => self.tag_name.push('\u{fffd}'),
                Some(ch) => self.tag_name.push(ch.to_ascii_lowercase()),
                None => self.eof(),
            },
            State::RcdataLessThan => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = State::RcdataEndTagOpen;
                }
                _ => {
                    self.emit_char('<');
                    self.reconsume(State::Rcdata);
                }
            },
            State::RcdataEndTagOpen | State::RawtextEndTagOpen | State::ScriptDataEndTagOpen | State::ScriptDataEscapedEndTagOpen => {
                let (name_state, back) = match self.state {
                    State::RcdataEndTagOpen => (State::RcdataEndTagName, State::Rcdata),
                    State::RawtextEndTagOpen => (State::RawtextEndTagName, State::Rawtext),
                    State::ScriptDataEndTagOpen => (State::ScriptDataEndTagName, State::ScriptData),
                    _ => (State::ScriptDataEscapedEndTagName, State::ScriptDataEscaped),
                };
                match c {
                    Some(ch) if ch.is_ascii_alphabetic() => {
                        self.new_tag(true);
                        self.reconsume(name_state);
                    }
                    _ => {
                        self.emit_str("</");
                        if c.is_some() {
                            self.reconsume(back);
                        } else {
                            self.pos -= 1;
                            self.state = back;
                        }
                    }
                }
            }
            State::RcdataEndTagName | State::RawtextEndTagName | State::ScriptDataEndTagName | State::ScriptDataEscapedEndTagName => {
                let back = match self.state {
                    State::RcdataEndTagName => State::Rcdata,
                    State::RawtextEndTagName => State::Rawtext,
                    State::ScriptDataEndTagName => State::ScriptData,
                    _ => State::ScriptDataEscaped,
                };
                match c {
                    Some(ch) if is_ws(ch) && self.appropriate_end_tag() => self.state = State::BeforeAttributeName,
                    Some('/') if self.appropriate_end_tag() => self.state = State::SelfClosingStartTag,
                    Some('>') if self.appropriate_end_tag() => {
                        self.state = State::Data;
                        self.emit_tag();
                    }
                    Some(ch) if ch.is_ascii_alphabetic() => {
                        self.tag_name.push(ch.to_ascii_lowercase());
                        self.temp.push(ch);
                    }
                    _ => {
                        self.emit_str("</");
                        let t = std::mem::take(&mut self.temp);
                        self.emit_str(&t);
                        if c.is_some() {
                            self.reconsume(back);
                        } else {
                            self.pos -= 1;
                            self.state = back;
                        }
                    }
                }
            }
            State::RawtextLessThan => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = State::RawtextEndTagOpen;
                }
                _ => {
                    self.emit_char('<');
                    self.back_or_eof(c, State::Rawtext);
                }
            },
            State::ScriptDataLessThan => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = State::ScriptDataEndTagOpen;
                }
                Some('!') => {
                    self.state = State::ScriptDataEscapeStart;
                    self.emit_str("<!");
                }
                _ => {
                    self.emit_char('<');
                    self.back_or_eof(c, State::ScriptData);
                }
            },
            State::ScriptDataEscapeStart => match c {
                Some('-') => {
                    self.state = State::ScriptDataEscapeStartDash;
                    self.emit_char('-');
                }
                _ => self.back_or_eof(c, State::ScriptData),
            },
            State::ScriptDataEscapeStartDash => match c {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                _ => self.back_or_eof(c, State::ScriptData),
            },
            State::ScriptDataEscaped => match c {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = State::ScriptDataEscapedLessThan,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::ScriptDataEscapedDash => match c {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = State::ScriptDataEscapedLessThan,
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(ch) => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char(ch);
                }
                None => self.eof(),
            },
            State::ScriptDataEscapedDashDash => match c {
                Some('-') => self.emit_char('-'),
                Some('<') => self.state = State::ScriptDataEscapedLessThan,
                Some('>') => {
                    self.state = State::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(ch) => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char(ch);
                }
                None => self.eof(),
            },
            State::ScriptDataEscapedLessThan => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = State::ScriptDataEscapedEndTagOpen;
                }
                Some(ch) if ch.is_ascii_alphabetic() => {
                    self.temp.clear();
                    self.emit_char('<');
                    self.reconsume(State::ScriptDataDoubleEscapeStart);
                }
                _ => {
                    self.emit_char('<');
                    self.back_or_eof(c, State::ScriptDataEscaped);
                }
            },
            State::ScriptDataDoubleEscapeStart | State::ScriptDataDoubleEscapeEnd => {
                let (on_script, other) = if self.state == State::ScriptDataDoubleEscapeStart {
                    (State::ScriptDataDoubleEscaped, State::ScriptDataEscaped)
                } else {
                    (State::ScriptDataEscaped, State::ScriptDataDoubleEscaped)
                };
                match c {
                    Some(ch) if is_ws(ch) || ch == '/' || ch == '>' => {
                        self.state = if self.temp == "script" { on_script } else { other };
                        self.emit_char(ch);
                    }
                    Some(ch) if ch.is_ascii_alphabetic() => {
                        self.temp.push(ch.to_ascii_lowercase());
                        self.emit_char(ch);
                    }
                    _ => {
                        let back = if self.state == State::ScriptDataDoubleEscapeStart { State::ScriptDataEscaped } else { State::ScriptDataDoubleEscaped };
                        self.back_or_eof(c, back);
                    }
                }
            }
            State::ScriptDataDoubleEscaped => match c {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::ScriptDataDoubleEscapedDash => match c {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(ch) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char(ch);
                }
                None => self.eof(),
            },
            State::ScriptDataDoubleEscapedDashDash => match c {
                Some('-') => self.emit_char('-'),
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThan;
                    self.emit_char('<');
                }
                Some('>') => {
                    self.state = State::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(ch) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char(ch);
                }
                None => self.eof(),
            },
            State::ScriptDataDoubleEscapedLessThan => match c {
                Some('/') => {
                    self.temp.clear();
                    self.state = State::ScriptDataDoubleEscapeEnd;
                    self.emit_char('/');
                }
                _ => self.back_or_eof(c, State::ScriptDataDoubleEscaped),
            },
            State::BeforeAttributeName => match c {
                Some(ch) if is_ws(ch) => {}
                Some('/') | Some('>') | None => self.back_or_eof(c, State::AfterAttributeName),
                Some('=') => {
                    self.start_attr();
                    self.attr_name.push('=');
                    self.state = State::AttributeName;
                }
                Some(_) => {
                    self.start_attr();
                    self.reconsume(State::AttributeName);
                }
            },
            State::AttributeName => match c {
                Some(ch) if is_ws(ch) || ch == '/' || ch == '>' => self.reconsume(State::AfterAttributeName),
                None => self.back_or_eof(c, State::AfterAttributeName),
                Some('=') => self.state = State::BeforeAttributeValue,
                Some('\0') => self.attr_name.push('\u{fffd}'),
                Some(ch) => self.attr_name.push(ch.to_ascii_lowercase()),
            },
            State::AfterAttributeName => match c {
                Some(ch) if is_ws(ch) => {}
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('=') => self.state = State::BeforeAttributeValue,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.eof(),
                Some(_) => {
                    self.start_attr();
                    self.reconsume(State::AttributeName);
                }
            },
            State::BeforeAttributeValue => match c {
                Some(ch) if is_ws(ch) => {}
                Some('"') => self.state = State::AttributeValueDoubleQuoted,
                Some('\'') => self.state = State::AttributeValueSingleQuoted,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                _ => self.back_or_eof(c, State::AttributeValueUnquoted),
            },
            State::AttributeValueDoubleQuoted | State::AttributeValueSingleQuoted => {
                let q = if self.state == State::AttributeValueDoubleQuoted { '"' } else { '\'' };
                match c {
                    Some(ch) if ch == q => self.state = State::AfterAttributeValueQuoted,
                    Some('&') => {
                        self.return_state = self.state;
                        self.state = State::CharacterReference;
                    }
                    Some('\0') => self.attr_value.push('\u{fffd}'),
                    Some(ch) => self.attr_value.push(ch),
                    None => self.eof(),
                }
            }
            State::AttributeValueUnquoted => match c {
                Some(ch) if is_ws(ch) => self.state = State::BeforeAttributeName,
                Some('&') => {
                    self.return_state = State::AttributeValueUnquoted;
                    self.state = State::CharacterReference;
                }
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                Some('\0') => self.attr_value.push('\u{fffd}'),
                Some(ch) => self.attr_value.push(ch),
                None => self.eof(),
            },
            State::AfterAttributeValueQuoted => match c {
                Some(ch) if is_ws(ch) => self.state = State::BeforeAttributeName,
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.eof(),
                Some(_) => self.reconsume(State::BeforeAttributeName),
            },
            State::SelfClosingStartTag => match c {
                Some('>') => {
                    self.tag_self_closing = true;
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.eof(),
                Some(_) => self.reconsume(State::BeforeAttributeName),
            },
            State::BogusComment => match c {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.eof();
                }
                Some('\0') => self.comment.push('\u{fffd}'),
                Some(ch) => self.comment.push(ch),
            },
            State::MarkupDeclarationOpen => {
                self.pos -= 1;
                if self.lookahead("--", false) {
                    self.pos += 2;
                    self.comment.clear();
                    self.state = State::CommentStart;
                } else if self.lookahead("doctype", true) {
                    self.pos += 7;
                    self.state = State::Doctype;
                } else if self.lookahead("[CDATA[", false) {
                    self.pos += 7;
                    if self.allow_cdata {
                        self.state = State::CdataSection;
                    } else {
                        self.comment.clear();
                        self.comment.push_str("[CDATA[");
                        self.state = State::BogusComment;
                    }
                } else {
                    self.comment.clear();
                    self.state = State::BogusComment;
                }
            }
            State::CommentStart => match c {
                Some('-') => self.state = State::CommentStartDash,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                _ => self.back_or_eof(c, State::Comment),
            },
            State::CommentStartDash => match c {
                Some('-') => self.state = State::CommentEnd,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(State::Comment);
                }
            },
            State::Comment => match c {
                Some('<') => {
                    self.comment.push('<');
                    self.state = State::CommentLessThan;
                }
                Some('-') => self.state = State::CommentEndDash,
                Some('\0') => self.comment.push('\u{fffd}'),
                Some(ch) => self.comment.push(ch),
                None => {
                    self.emit_comment();
                    self.eof();
                }
            },
            State::CommentLessThan => match c {
                Some('!') => {
                    self.comment.push('!');
                    self.state = State::CommentLessThanBang;
                }
                Some('<') => self.comment.push('<'),
                _ => self.back_or_eof(c, State::Comment),
            },
            State::CommentLessThanBang => match c {
                Some('-') => self.state = State::CommentLessThanBangDash,
                _ => self.back_or_eof(c, State::Comment),
            },
            State::CommentLessThanBangDash => match c {
                Some('-') => self.state = State::CommentLessThanBangDashDash,
                _ => self.back_or_eof(c, State::CommentEndDash),
            },
            State::CommentLessThanBangDashDash => self.back_or_eof(c, State::CommentEnd),
            State::CommentEndDash => match c {
                Some('-') => self.state = State::CommentEnd,
                None => {
                    self.emit_comment();
                    self.eof();
                }
                Some(_) => {
                    self.comment.push('-');
                    self.reconsume(State::Comment);
                }
            },
            State::CommentEnd => match c {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                Some('!') => self.state = State::CommentEndBang,
                Some('-') => self.comment.push('-'),
                None => {
                    self.emit_comment();
                    self.eof();
                }
                Some(_) => {
                    self.comment.push_str("--");
                    self.reconsume(State::Comment);
                }
            },
            State::CommentEndBang => match c {
                Some('-') => {
                    self.comment.push_str("--!");
                    self.state = State::CommentEndDash;
                }
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.eof();
                }
                Some(_) => {
                    self.comment.push_str("--!");
                    self.reconsume(State::Comment);
                }
            },
            State::Doctype => match c {
                Some(ch) if is_ws(ch) => self.state = State::BeforeDoctypeName,
                Some('>') => self.reconsume(State::BeforeDoctypeName),
                None => {
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.eof();
                }
                Some(_) => self.reconsume(State::BeforeDoctypeName),
            },
            State::BeforeDoctypeName => match c {
                Some(ch) if is_ws(ch) => {}
                Some('>') => {
                    self.doctype.force_quirks = true;
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.eof();
                }
                Some(ch) => {
                    self.doctype.name = Some(if ch == '\0' { '\u{fffd}'.to_string() } else { ch.to_ascii_lowercase().to_string() });
                    self.state = State::DoctypeName;
                }
            },
            State::DoctypeName => match c {
                Some(ch) if is_ws(ch) => self.state = State::AfterDoctypeName,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.eof();
                }
                Some(ch) => {
                    let n = self.doctype.name.get_or_insert_with(String::new);
                    n.push(if ch == '\0' { '\u{fffd}' } else { ch.to_ascii_lowercase() });
                }
            },
            State::AfterDoctypeName => match c {
                Some(ch) if is_ws(ch) => {}
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.eof();
                }
                Some(_) => {
                    self.pos -= 1;
                    if self.lookahead("public", true) {
                        self.pos += 6;
                        self.state = State::AfterDoctypePublicKeyword;
                    } else if self.lookahead("system", true) {
                        self.pos += 6;
                        self.state = State::AfterDoctypeSystemKeyword;
                    } else {
                        self.pos += 1;
                        self.doctype.force_quirks = true;
                        self.reconsume(State::BogusDoctype);
                    }
                }
            },
            State::AfterDoctypePublicKeyword | State::AfterDoctypeSystemKeyword => {
                let public = self.state == State::AfterDoctypePublicKeyword;
                match c {
                    Some(ch) if is_ws(ch) => self.state = if public { State::BeforeDoctypePublicId } else { State::BeforeDoctypeSystemId },
                    Some(q @ ('"' | '\'')) => self.open_id(public, q),
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = State::Data;
                        self.emit_doctype();
                    }
                    None => {
                        self.doctype.force_quirks = true;
                        self.emit_doctype();
                        self.eof();
                    }
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(State::BogusDoctype);
                    }
                }
            }
            State::BeforeDoctypePublicId | State::BeforeDoctypeSystemId => {
                let public = self.state == State::BeforeDoctypePublicId;
                match c {
                    Some(ch) if is_ws(ch) => {}
                    Some(q @ ('"' | '\'')) => self.open_id(public, q),
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = State::Data;
                        self.emit_doctype();
                    }
                    None => {
                        self.doctype.force_quirks = true;
                        self.emit_doctype();
                        self.eof();
                    }
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(State::BogusDoctype);
                    }
                }
            }
            State::DoctypePublicIdDoubleQuoted | State::DoctypePublicIdSingleQuoted | State::DoctypeSystemIdDoubleQuoted | State::DoctypeSystemIdSingleQuoted => {
                let (public, q) = match self.state {
                    State::DoctypePublicIdDoubleQuoted => (true, '"'),
                    State::DoctypePublicIdSingleQuoted => (true, '\''),
                    State::DoctypeSystemIdDoubleQuoted => (false, '"'),
                    _ => (false, '\''),
                };
                match c {
                    Some(ch) if ch == q => self.state = if public { State::AfterDoctypePublicId } else { State::AfterDoctypeSystemId },
                    Some('>') => {
                        self.doctype.force_quirks = true;
                        self.state = State::Data;
                        self.emit_doctype();
                    }
                    None => {
                        self.doctype.force_quirks = true;
                        self.emit_doctype();
                        self.eof();
                    }
                    Some(ch) => {
                        let ch = if ch == '\0' { '\u{fffd}' } else { ch };
                        let id = if public { &mut self.doctype.public_id } else { &mut self.doctype.system_id };
                        id.get_or_insert_with(String::new).push(ch);
                    }
                }
            }
            State::AfterDoctypePublicId | State::BetweenDoctypePublicAndSystem => {
                let after = self.state == State::AfterDoctypePublicId;
                match c {
                    Some(ch) if is_ws(ch) => {
                        if after {
                            self.state = State::BetweenDoctypePublicAndSystem;
                        }
                    }
                    Some('>') => {
                        self.state = State::Data;
                        self.emit_doctype();
                    }
                    Some(q @ ('"' | '\'')) => self.open_id(false, q),
                    None => {
                        self.doctype.force_quirks = true;
                        self.emit_doctype();
                        self.eof();
                    }
                    Some(_) => {
                        self.doctype.force_quirks = true;
                        self.reconsume(State::BogusDoctype);
                    }
                }
            }
            State::AfterDoctypeSystemId => match c {
                Some(ch) if is_ws(ch) => {}
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype.force_quirks = true;
                    self.emit_doctype();
                    self.eof();
                }
                Some(_) => self.reconsume(State::BogusDoctype),
            },
            State::BogusDoctype => match c {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.emit_doctype();
                    self.eof();
                }
                Some(_) => {}
            },
            State::CdataSection => match c {
                Some(']') => self.state = State::CdataSectionBracket,
                Some(ch) => self.emit_char(ch),
                None => self.eof(),
            },
            State::CdataSectionBracket => match c {
                Some(']') => self.state = State::CdataSectionEnd,
                _ => {
                    self.emit_char(']');
                    self.back_or_eof(c, State::CdataSection);
                }
            },
            State::CdataSectionEnd => match c {
                Some(']') => self.emit_char(']'),
                Some('>') => self.state = State::Data,
                _ => {
                    self.emit_str("]]");
                    self.back_or_eof(c, State::CdataSection);
                }
            },
            State::CharacterReference => {
                self.temp.clear();
                self.temp.push('&');
                match c {
                    Some(ch) if ch.is_ascii_alphanumeric() => self.reconsume(State::NamedCharacterReference),
                    Some('#') => {
                        self.temp.push('#');
                        self.state = State::NumericCharacterReference;
                    }
                    _ => {
                        self.flush_temp();
                        let r = self.return_state;
                        self.back_or_eof(c, r);
                    }
                }
            }
            State::NamedCharacterReference => {
                self.pos -= 1;
                let rest: String = self.input[self.pos..self.input.len().min(self.pos + 48)].iter().collect();
                match entities::match_prefix(&rest) {
                    Some((used, value)) => {
                        let matched = &rest[..used];
                        let semi = matched.ends_with(';');
                        let next = rest[used..].chars().next();
                        self.pos += matched.chars().count();
                        if self.charref_in_attr() && !semi && next.map(|n| n == '=' || n.is_ascii_alphanumeric()).unwrap_or(false) {
                            self.temp.push_str(matched);
                            self.flush_temp();
                            self.state = self.return_state;
                        } else {
                            self.temp.clear();
                            self.temp.push_str(value);
                            self.flush_temp();
                            self.state = self.return_state;
                        }
                    }
                    None => {
                        self.flush_temp();
                        self.state = State::AmbiguousAmpersand;
                    }
                }
            }
            State::AmbiguousAmpersand => match c {
                Some(ch) if ch.is_ascii_alphanumeric() => {
                    if self.charref_in_attr() {
                        self.attr_value.push(ch);
                    } else {
                        self.emit_char(ch);
                    }
                }
                _ => {
                    let r = self.return_state;
                    self.back_or_eof(c, r);
                }
            },
            State::NumericCharacterReference => {
                self.char_ref_code = 0;
                match c {
                    Some(ch @ ('x' | 'X')) => {
                        self.temp.push(ch);
                        self.state = State::HexadecimalCharacterReferenceStart;
                    }
                    _ => self.back_or_eof(c, State::DecimalCharacterReferenceStart),
                }
            }
            State::HexadecimalCharacterReferenceStart | State::DecimalCharacterReferenceStart => {
                let hex = self.state == State::HexadecimalCharacterReferenceStart;
                match c {
                    Some(ch) if (hex && ch.is_ascii_hexdigit()) || (!hex && ch.is_ascii_digit()) => {
                        self.reconsume(if hex { State::HexadecimalCharacterReference } else { State::DecimalCharacterReference })
                    }
                    _ => {
                        self.flush_temp();
                        let r = self.return_state;
                        self.back_or_eof(c, r);
                    }
                }
            }
            State::HexadecimalCharacterReference | State::DecimalCharacterReference => {
                let radix = if self.state == State::HexadecimalCharacterReference { 16 } else { 10 };
                match c {
                    Some(ch) if ch.is_digit(radix) => {
                        self.char_ref_code = self.char_ref_code.saturating_mul(radix).saturating_add(ch.to_digit(radix).unwrap()).min(0x110000);
                    }
                    Some(';') => self.state = State::NumericCharacterReferenceEnd,
                    _ => self.back_or_eof(c, State::NumericCharacterReferenceEnd),
                }
            }
            State::NumericCharacterReferenceEnd => {
                // Consumes nothing.
                self.pos -= 1;
                let ch = entities::numeric(self.char_ref_code);
                self.temp.clear();
                self.temp.push(ch);
                self.flush_temp();
                self.state = self.return_state;
            }
        }
    }

    /// Reconsume `c` in `state` (or, at EOF, stay at EOF in `state`).
    fn back_or_eof(&mut self, _c: Option<char>, state: State) {
        self.pos -= 1;
        self.state = state;
    }

    fn open_id(&mut self, public: bool, q: char) {
        if public {
            self.doctype.public_id = Some(String::new());
            self.state = if q == '"' { State::DoctypePublicIdDoubleQuoted } else { State::DoctypePublicIdSingleQuoted };
        } else {
            self.doctype.system_id = Some(String::new());
            self.state = if q == '"' { State::DoctypeSystemIdDoubleQuoted } else { State::DoctypeSystemIdSingleQuoted };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &str) -> Vec<Token> {
        let mut t = Tokenizer::new(input);
        let mut out = Vec::new();
        loop {
            let tok = t.next_token();
            if tok == Token::Eof {
                break;
            }
            if let Token::StartTag { name, .. } = &tok {
                if name == "script" {
                    t.set_raw(RawKind::ScriptData, "script");
                }
            }
            out.push(tok);
        }
        out
    }

    #[test]
    fn basic() {
        let t = all("<p class=a id='b' x=\"&amp;c\">hi &lt; there</p><!-- c --><!DOCTYPE html>");
        assert_eq!(
            t,
            vec![
                Token::StartTag { name: "p".into(), attrs: vec![("class".into(), "a".into()), ("id".into(), "b".into()), ("x".into(), "&c".into())], self_closing: false },
                Token::Text("hi < there".into()),
                Token::EndTag { name: "p".into() },
                Token::Comment(" c ".into()),
                Token::Doctype(Doctype { name: Some("html".into()), public_id: None, system_id: None, force_quirks: false }),
            ]
        );
    }

    #[test]
    fn script_raw() {
        let t = all("<script>if (a<b) x='</p>'</script>");
        assert_eq!(t[1], Token::Text("if (a<b) x='</p>'".into()));
        assert_eq!(t[2], Token::EndTag { name: "script".into() });
    }

    #[test]
    fn stray_lt_and_refs() {
        assert_eq!(all("a < b"), vec![Token::Text("a < b".into())]);
        assert_eq!(all("&notit; &#x41;&#0;"), vec![Token::Text("¬it; A\u{fffd}".into())]);
        assert_eq!(all("<a href='?a=1&copy=2'>")[0], Token::StartTag { name: "a".into(), attrs: vec![("href".into(), "?a=1&copy=2".into())], self_closing: false });
    }
}
