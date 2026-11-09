//! CSS counters (CSS 2.1 §12.4) and quote nesting state, tracked in tree
//! order while the box tree is built.

use rabbithole_style::ListStyleType;

/// Counter instances organised per element nesting level. An instance
/// created by `counter-reset` on an element lives in its parent's frame so
/// it is visible to the element, its descendants and its following
/// siblings; it disappears when the parent is left.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    frames: Vec<Vec<(String, i32)>>,
}

impl Counters {
    pub fn new() -> Self {
        Counters { frames: vec![Vec::new()] }
    }

    /// Enter an element (its children's resets go into the new frame).
    pub fn enter(&mut self) {
        self.frames.push(Vec::new());
    }

    pub fn leave(&mut self) {
        if self.frames.len() > 1 {
            self.frames.pop();
        }
    }

    /// `counter-reset: name value` on the current element.
    pub fn reset(&mut self, name: &str, value: i32) {
        if let Some(top) = self.frames.last_mut() {
            top.push((name.to_string(), value));
        }
    }

    /// `counter-increment: name by`.
    pub fn increment(&mut self, name: &str, by: i32) {
        for frame in self.frames.iter_mut().rev() {
            if let Some(entry) = frame.iter_mut().rev().find(|(n, _)| n == name) {
                entry.1 = entry.1.saturating_add(by);
                return;
            }
        }
        // No instance in scope: implicitly reset to 0 on the current frame.
        if let Some(top) = self.frames.last_mut() {
            top.push((name.to_string(), by));
        }
    }

    /// Innermost value of `name` (0 when none).
    pub fn value(&self, name: &str) -> i32 {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.iter().rev().find(|(n, _)| n == name).map(|(_, v)| *v))
            .unwrap_or(0)
    }

    /// All instances of `name` in scope, outermost first (for `counters()`).
    pub fn values(&self, name: &str) -> Vec<i32> {
        let mut out = Vec::new();
        for frame in &self.frames {
            for (n, v) in frame {
                if n == name {
                    out.push(*v);
                }
            }
        }
        out
    }
}

/// Format a counter value in a list style.
pub(crate) fn format_counter(value: i32, style: ListStyleType) -> String {
    let n = value as i64;
    match style {
        ListStyleType::None => String::new(),
        ListStyleType::Disc => "•".into(),
        ListStyleType::Circle => "◦".into(),
        ListStyleType::Square => "▪".into(),
        ListStyleType::Decimal => n.to_string(),
        ListStyleType::DecimalLeadingZero => {
            if n < 0 {
                format!("-{:02}", -n)
            } else {
                format!("{n:02}")
            }
        }
        ListStyleType::LowerAlpha => alpha(n, false),
        ListStyleType::UpperAlpha => alpha(n, true),
        ListStyleType::LowerRoman => roman(n).to_lowercase(),
        ListStyleType::UpperRoman => roman(n),
    }
}

pub(crate) fn alpha(mut n: i64, upper: bool) -> String {
    if n <= 0 {
        return n.to_string();
    }
    let mut s = Vec::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        s.push(if upper { b'A' + r } else { b'a' + r });
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

pub(crate) fn roman(mut n: i64) -> String {
    if n <= 0 || n >= 4000 {
        return n.to_string();
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
        (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in TABLE {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Quote nesting depth for `open-quote` / `close-quote`.
#[derive(Debug, Default)]
pub(crate) struct Quotes {
    pub depth: usize,
}

const DEFAULT_QUOTES: [(&str, &str); 2] = [("\u{201c}", "\u{201d}"), ("\u{2018}", "\u{2019}")];

impl Quotes {
    fn pair(pairs: &[(String, String)], depth: usize) -> (String, String) {
        if pairs.is_empty() {
            let (o, c) = DEFAULT_QUOTES[depth.min(DEFAULT_QUOTES.len() - 1)];
            (o.to_string(), c.to_string())
        } else {
            pairs[depth.min(pairs.len() - 1)].clone()
        }
    }

    pub fn open(&mut self, pairs: &[(String, String)]) -> String {
        let (o, _) = Self::pair(pairs, self.depth);
        self.depth += 1;
        o
    }

    pub fn close(&mut self, pairs: &[(String, String)]) -> String {
        self.depth = self.depth.saturating_sub(1);
        let (_, c) = Self::pair(pairs, self.depth);
        c
    }

    pub fn no_open(&mut self) {
        self.depth += 1;
    }

    pub fn no_close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_scopes() {
        let mut c = Counters::new();
        c.reset("s", 0); // <body counter-reset>
        c.enter();
        c.increment("s", 1);
        assert_eq!(c.value("s"), 1);
        c.enter(); // nested element with its own reset
        c.reset("s", 10);
        c.increment("s", 1);
        assert_eq!(c.value("s"), 11);
        assert_eq!(c.values("s"), vec![1, 11]);
        c.leave();
        c.increment("s", 1);
        assert_eq!(c.value("s"), 2);
        c.leave();
    }

    #[test]
    fn formats() {
        assert_eq!(alpha(1, false), "a");
        assert_eq!(alpha(27, true), "AA");
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(format_counter(7, ListStyleType::DecimalLeadingZero), "07");
    }

    #[test]
    fn quotes_nest() {
        let mut q = Quotes::default();
        assert_eq!(q.open(&[]), "\u{201c}");
        assert_eq!(q.open(&[]), "\u{2018}");
        assert_eq!(q.close(&[]), "\u{2019}");
        assert_eq!(q.close(&[]), "\u{201d}");
    }
}
