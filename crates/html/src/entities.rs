//! Named character references. This is the commonly used subset of the
//! HTML5 entity table; numeric references are handled separately.

pub fn lookup(name: &str) -> Option<&'static str> {
    // Binary search over a sorted table.
    ENTITIES
        .binary_search_by(|(k, _)| k.cmp(&name))
        .ok()
        .map(|i| ENTITIES[i].1)
}

// Must stay sorted by name (byte order) for the binary search above.
static ENTITIES: &[(&str, &str)] = &[
    ("AElig", "Æ"), ("AMP", "&"), ("Aacute", "Á"), ("Acirc", "Â"), ("Agrave", "À"),
    ("Alpha", "Α"), ("Aring", "Å"), ("Atilde", "Ã"), ("Auml", "Ä"), ("Beta", "Β"),
    ("COPY", "©"), ("Ccedil", "Ç"), ("Chi", "Χ"), ("Dagger", "‡"), ("Delta", "Δ"),
    ("ETH", "Ð"), ("Eacute", "É"), ("Ecirc", "Ê"), ("Egrave", "È"), ("Epsilon", "Ε"),
    ("Eta", "Η"), ("Euml", "Ë"), ("GT", ">"), ("Gamma", "Γ"), ("Iacute", "Í"),
    ("Icirc", "Î"), ("Igrave", "Ì"), ("Iota", "Ι"), ("Iuml", "Ï"), ("Kappa", "Κ"),
    ("LT", "<"), ("Lambda", "Λ"), ("Mu", "Μ"), ("Ntilde", "Ñ"), ("Nu", "Ν"),
    ("OElig", "Œ"), ("Oacute", "Ó"), ("Ocirc", "Ô"), ("Ograve", "Ò"), ("Omega", "Ω"),
    ("Omicron", "Ο"), ("Oslash", "Ø"), ("Otilde", "Õ"), ("Ouml", "Ö"), ("Phi", "Φ"),
    ("Pi", "Π"), ("Prime", "″"), ("Psi", "Ψ"), ("QUOT", "\""), ("REG", "®"),
    ("Rho", "Ρ"), ("Scaron", "Š"), ("Sigma", "Σ"), ("THORN", "Þ"), ("Tau", "Τ"),
    ("Theta", "Θ"), ("Uacute", "Ú"), ("Ucirc", "Û"), ("Ugrave", "Ù"), ("Upsilon", "Υ"),
    ("Uuml", "Ü"), ("Xi", "Ξ"), ("Yacute", "Ý"), ("Yuml", "Ÿ"), ("Zeta", "Ζ"),
    ("aacute", "á"), ("acirc", "â"), ("acute", "´"), ("aelig", "æ"), ("agrave", "à"),
    ("alefsym", "ℵ"), ("alpha", "α"), ("amp", "&"), ("and", "∧"), ("ang", "∠"),
    ("apos", "'"), ("aring", "å"), ("asymp", "≈"), ("atilde", "ã"), ("auml", "ä"),
    ("bdquo", "„"), ("beta", "β"), ("brvbar", "¦"), ("bull", "•"), ("bullet", "•"),
    ("cap", "∩"), ("ccedil", "ç"), ("cedil", "¸"), ("cent", "¢"), ("check", "✓"),
    ("checkmark", "✓"), ("chi", "χ"), ("circ", "ˆ"), ("clubs", "♣"), ("colon", ":"),
    ("comma", ","), ("cong", "≅"), ("copy", "©"), ("crarr", "↵"), ("cross", "✗"),
    ("cup", "∪"), ("curren", "¤"), ("dArr", "⇓"), ("dagger", "†"), ("darr", "↓"),
    ("dash", "‐"), ("deg", "°"), ("delta", "δ"), ("diams", "♦"), ("divide", "÷"),
    ("dollar", "$"), ("eacute", "é"), ("ecirc", "ê"), ("egrave", "è"), ("empty", "∅"),
    ("emsp", "\u{2003}"), ("ensp", "\u{2002}"), ("epsilon", "ε"), ("equals", "="), ("equiv", "≡"),
    ("eta", "η"), ("eth", "ð"), ("euml", "ë"), ("euro", "€"), ("excl", "!"),
    ("exist", "∃"), ("fnof", "ƒ"), ("forall", "∀"), ("frac12", "½"), ("frac14", "¼"),
    ("frac34", "¾"), ("frasl", "⁄"), ("gamma", "γ"), ("ge", "≥"), ("gt", ">"),
    ("hArr", "⇔"), ("half", "½"), ("harr", "↔"), ("hearts", "♥"), ("hellip", "…"),
    ("hyphen", "‐"), ("iacute", "í"), ("icirc", "î"), ("iexcl", "¡"), ("igrave", "ì"),
    ("image", "ℑ"), ("infin", "∞"), ("int", "∫"), ("iota", "ι"), ("iquest", "¿"),
    ("isin", "∈"), ("iuml", "ï"), ("kappa", "κ"), ("lArr", "⇐"), ("lambda", "λ"),
    ("lang", "⟨"), ("laquo", "«"), ("larr", "←"), ("lbrace", "{"), ("lbrack", "["),
    ("lceil", "⌈"), ("ldquo", "“"), ("le", "≤"), ("lfloor", "⌊"), ("lowast", "∗"),
    ("loz", "◊"), ("lpar", "("), ("lrm", "\u{200E}"), ("lsaquo", "‹"), ("lsqb", "["),
    ("lsquo", "‘"), ("lt", "<"), ("macr", "¯"), ("mdash", "—"), ("micro", "µ"),
    ("middot", "·"), ("minus", "−"), ("mu", "μ"), ("nabla", "∇"), ("nbsp", "\u{a0}"),
    ("ndash", "–"), ("ne", "≠"), ("ni", "∋"), ("not", "¬"), ("notin", "∉"),
    ("nsub", "⊄"), ("ntilde", "ñ"), ("nu", "ν"), ("num", "#"), ("oacute", "ó"),
    ("ocirc", "ô"), ("oelig", "œ"), ("ograve", "ò"), ("oline", "‾"), ("omega", "ω"),
    ("omicron", "ο"), ("oplus", "⊕"), ("or", "∨"), ("ordf", "ª"), ("ordm", "º"),
    ("oslash", "ø"), ("otilde", "õ"), ("otimes", "⊗"), ("ouml", "ö"), ("para", "¶"),
    ("part", "∂"), ("percnt", "%"), ("period", "."), ("permil", "‰"), ("perp", "⊥"),
    ("phi", "φ"), ("pi", "π"), ("piv", "ϖ"), ("plus", "+"), ("plusmn", "±"),
    ("pound", "£"), ("prime", "′"), ("prod", "∏"), ("prop", "∝"), ("psi", "ψ"),
    ("quest", "?"), ("quot", "\""), ("rArr", "⇒"), ("radic", "√"), ("rang", "⟩"),
    ("raquo", "»"), ("rarr", "→"), ("rbrace", "}"), ("rbrack", "]"), ("rceil", "⌉"),
    ("rdquo", "”"), ("real", "ℜ"), ("reg", "®"), ("rfloor", "⌋"), ("rho", "ρ"),
    ("rlm", "\u{200F}"), ("rpar", ")"), ("rsaquo", "›"), ("rsqb", "]"), ("rsquo", "’"),
    ("sbquo", "‚"), ("scaron", "š"), ("sdot", "⋅"), ("sect", "§"), ("semi", ";"),
    ("shy", "\u{AD}"), ("sigma", "σ"), ("sigmaf", "ς"), ("sim", "∼"), ("sol", "/"),
    ("spades", "♠"), ("star", "☆"), ("starf", "★"), ("sub", "⊂"), ("sube", "⊆"),
    ("sum", "∑"), ("sup", "⊃"), ("sup1", "¹"), ("sup2", "²"), ("sup3", "³"),
    ("supe", "⊇"), ("szlig", "ß"), ("tau", "τ"), ("there4", "∴"), ("theta", "θ"),
    ("thetasym", "ϑ"), ("thinsp", "\u{2009}"), ("thorn", "þ"), ("tilde", "˜"), ("times", "×"),
    ("trade", "™"), ("uArr", "⇑"), ("uacute", "ú"), ("uarr", "↑"), ("ucirc", "û"),
    ("ugrave", "ù"), ("uml", "¨"), ("upsih", "ϒ"), ("upsilon", "υ"), ("uuml", "ü"),
    ("verbar", "|"), ("vert", "|"), ("weierp", "℘"), ("xi", "ξ"), ("yacute", "ý"),
    ("yen", "¥"), ("yuml", "ÿ"), ("zeta", "ζ"), ("zwj", "\u{200D}"), ("zwnj", "\u{200C}"),
];

/// Decode a numeric reference code point, applying the HTML5 replacements
/// for the Windows-1252 range and invalid code points.
pub fn numeric(cp: u32) -> char {
    const WIN1252: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{9d}', 'ž', 'Ÿ',
    ];
    if (0x80..=0x9f).contains(&cp) {
        return WIN1252[(cp - 0x80) as usize];
    }
    if cp == 0 || cp > 0x10ffff || (0xd800..=0xdfff).contains(&cp) {
        return '\u{fffd}';
    }
    char::from_u32(cp).unwrap_or('\u{fffd}')
}

/// Decode character references in `input`. `in_attribute` enables the
/// HTML rule that `&name` followed by `=` or an alphanumeric inside an
/// attribute is left alone (so URLs like `?a=1&copy=2` survive).
pub fn decode(input: &str, in_attribute: bool) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            // Copy a run of plain bytes.
            let start = i;
            while i < bytes.len() && bytes[i] != b'&' {
                i += 1;
            }
            out.push_str(&input[start..i]);
            continue;
        }
        // At '&'
        let rest = &bytes[i + 1..];
        if rest.first() == Some(&b'#') {
            let (hex, mut j) = match rest.get(1) {
                Some(b'x') | Some(b'X') => (true, 2),
                _ => (false, 1),
            };
            let num_start = j;
            while j < rest.len()
                && (if hex { rest[j].is_ascii_hexdigit() } else { rest[j].is_ascii_digit() })
            {
                j += 1;
            }
            if j == num_start {
                out.push('&');
                i += 1;
                continue;
            }
            let digits = &input[i + 1 + num_start..i + 1 + j];
            let cp = u32::from_str_radix(digits, if hex { 16 } else { 10 }).unwrap_or(0xfffd);
            out.push(numeric(cp));
            i += 1 + j;
            if i < bytes.len() && bytes[i] == b';' {
                i += 1;
            }
            continue;
        }
        // Named: longest alphanumeric run (max 32 chars)
        let mut j = 0;
        while j < rest.len() && j < 32 && rest[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j == 0 {
            out.push('&');
            i += 1;
            continue;
        }
        let name = &input[i + 1..i + 1 + j];
        let has_semicolon = rest.get(j) == Some(&b';');
        let mut matched = None;
        if let Some(v) = lookup(name) {
            matched = Some((v, j));
        } else {
            // Try shorter prefixes for legacy no-semicolon entities (e.g. &copyright -> ©right)
            if !has_semicolon {
                for k in (2..j).rev() {
                    if let Some(v) = lookup(&name[..k]) {
                        matched = Some((v, k));
                        break;
                    }
                }
            }
        }
        match matched {
            Some((v, used)) => {
                let followed_by_semi = rest.get(used) == Some(&b';');
                if !followed_by_semi && in_attribute {
                    // In attributes, legacy entities not followed by ';' are
                    // not decoded when followed by '=' or alphanumerics.
                    if let Some(&c) = rest.get(used) {
                        if c == b'=' || c.is_ascii_alphanumeric() {
                            out.push('&');
                            i += 1;
                            continue;
                        }
                    }
                }
                out.push_str(v);
                i += 1 + used;
                if followed_by_semi {
                    i += 1;
                }
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_is_sorted() {
        for w in ENTITIES.windows(2) {
            assert!(w[0].0 < w[1].0, "{} >= {}", w[0].0, w[1].0);
        }
    }
    #[test]
    fn decodes() {
        assert_eq!(decode("a &amp; b &lt;c&gt; &#65;&#x42; &nbsp;x &copy &foo;", false), "a & b <c> AB \u{a0}x © &foo;");
        assert_eq!(decode("?a=1&copy=2", true), "?a=1&copy=2");
        assert_eq!(decode("?a=1&copy;=2", true), "?a=1©=2");
    }
}
