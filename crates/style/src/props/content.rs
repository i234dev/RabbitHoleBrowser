//! Generated content: `content`, counters and `quotes`.

use super::ApplyCtx;
use crate::computed::*;
use crate::values;
use rabbithole_css::Token;

fn list_style(kw: Option<&[Token]>) -> ListStyleType {
    kw.and_then(values::ident)
        .and_then(|k| crate::cascade::list_style_type(&k))
        .unwrap_or(ListStyleType::Decimal)
}

fn parse_content(comps: &[&[Token]], ctx: &ApplyCtx) -> Option<Content> {
    if comps.len() == 1 {
        if values::is_ident(comps[0], "normal") {
            return Some(Content::Normal);
        }
        if values::is_ident(comps[0], "none") {
            return Some(Content::None);
        }
    }
    let mut items = Vec::new();
    for c in comps {
        match c {
            [Token::String(s)] => items.push(ContentItem::String(s.clone())),
            [Token::Ident(i)] => match i.to_ascii_lowercase().as_str() {
                "open-quote" => items.push(ContentItem::OpenQuote),
                "close-quote" => items.push(ContentItem::CloseQuote),
                "no-open-quote" => items.push(ContentItem::NoOpenQuote),
                "no-close-quote" => items.push(ContentItem::NoCloseQuote),
                "normal" | "none" => return None,
                _ => return None,
            },
            [Token::Url(u)] => items.push(ContentItem::Url(ctx.url(u))),
            _ => {
                let name = super::fn_name(c)?;
                let args = super::fn_args(c);
                match name.as_str() {
                    "url" => {
                        let u = crate::cascade::url_of(c)?;
                        items.push(ContentItem::Url(ctx.url(&u)));
                    }
                    "attr" => {
                        let a = args.first()?.first()?;
                        let n = match a {
                            [Token::Ident(n)] | [Token::String(n)] => n.to_ascii_lowercase(),
                            _ => return None,
                        };
                        items.push(ContentItem::Attr(n));
                    }
                    "counter" => {
                        let n = values::ident(args.first()?.first()?)?;
                        let style = list_style(args.get(1).and_then(|a| a.first().copied()));
                        items.push(ContentItem::Counter(n, style));
                    }
                    "counters" => {
                        let n = values::ident(args.first()?.first()?)?;
                        let sep = match args.get(1)?.first()? {
                            [Token::String(s)] => s.clone(),
                            _ => return None,
                        };
                        let style = list_style(args.get(2).and_then(|a| a.first().copied()));
                        items.push(ContentItem::Counters(n, sep, style));
                    }
                    "image-set" | "-webkit-image-set" | "linear-gradient" | "radial-gradient" | "var" => {}
                    _ => return None,
                }
            }
        }
    }
    Some(Content::Items(items))
}

fn parse_counters(comps: &[&[Token]], default: i32) -> Option<Vec<(String, i32)>> {
    if comps.len() == 1 && values::is_ident(comps[0], "none") {
        return Some(Vec::new());
    }
    let mut out: Vec<(String, i32)> = Vec::new();
    for c in comps {
        if let Some(n) = values::number(c) {
            if let Some(last) = out.last_mut() {
                last.1 = n as i32;
            } else {
                return None;
            }
        } else if let [Token::Ident(name)] = c {
            out.push((name.clone(), default));
        } else {
            return None;
        }
    }
    Some(out)
}

pub fn apply(name: &str, comps: &[&[Token]], style: &mut ComputedStyle, ctx: &ApplyCtx) -> bool {
    match name {
        "content" => {
            if !ctx.pseudo {
                return true;
            }
            if let Some(c) = parse_content(comps, ctx) {
                style.content = c;
            }
        }
        "counter-reset" | "counter-set" => {
            if let Some(v) = parse_counters(comps, 0) {
                style.counter_reset = v;
            }
        }
        "counter-increment" => {
            if let Some(v) = parse_counters(comps, 1) {
                style.counter_increment = v;
            }
        }
        "quotes" => {
            if comps.len() == 1 {
                if values::is_ident(comps[0], "none") {
                    style.quotes = Vec::new();
                    return true;
                }
                if values::is_ident(comps[0], "auto") {
                    style.quotes = vec![("\u{201C}".into(), "\u{201D}".into()), ("\u{2018}".into(), "\u{2019}".into())];
                    return true;
                }
            }
            let strs: Vec<String> = comps
                .iter()
                .filter_map(|c| match c {
                    [Token::String(s)] => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if strs.len() == comps.len() && strs.len() % 2 == 0 {
                style.quotes = strs.chunks(2).map(|p| (p[0].clone(), p[1].clone())).collect();
            }
        }
        _ => return false,
    }
    true
}
