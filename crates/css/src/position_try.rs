//! The descriptor grammar shared by @position-try stylesheet rules and CSSOM.
use crate::Token;

pub const DESCRIPTORS: &[&str] = &[
    "top", "right", "bottom", "left", "inset", "inset-block", "inset-inline",
    "inset-block-start", "inset-block-end", "inset-inline-start", "inset-inline-end",
    "margin", "margin-top", "margin-right", "margin-bottom", "margin-left",
    "margin-block", "margin-inline", "margin-block-start", "margin-block-end",
    "margin-inline-start", "margin-inline-end", "width", "height", "block-size", "inline-size",
    "min-width", "min-height", "min-block-size", "min-inline-size",
    "max-width", "max-height", "max-block-size", "max-inline-size", "align-self",
    "justify-self", "place-self", "position-anchor", "position-area",
];

pub fn name(tokens: &[Token]) -> Option<&str> {
    let mut tokens = tokens.iter().filter(|t| !matches!(t, Token::Whitespace));
    match (tokens.next(), tokens.next()) {
        (Some(Token::Ident(name)), None) if name.starts_with("--") && name.len() > 2 => Some(name),
        _ => None,
    }
}
