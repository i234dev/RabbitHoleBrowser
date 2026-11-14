//! Bounded GLSL ES token preprocessing, before the shader grammar/type checker.
//! GLSL ES 1.00 §3.4 and 3.00 §3.1/3.4/3.5 define the implemented rules.
//! Unsigned conditional arithmetic, #line and extensions remain explicit errors.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

const MAX_BYTES: usize = 64 * 1024;
const MAX_TOKENS: usize = 16_384;
const MAX_MACROS: usize = 256;
const MAX_ARGUMENTS: usize = 32;
const MAX_EXPANSION_DEPTH: usize = 32;
const MAX_CONDITIONAL_DEPTH: usize = 64;
const MAX_WORK: usize = 131_072;
const MAX_CREATED_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: usize,
    space: bool,
    hidden: BTreeSet<usize>,
    depth: usize,
}

impl Token {
    fn new(text: &str, line: usize, space: bool) -> Self {
        Self { text: text.into(), line, space, hidden: BTreeSet::new(), depth: 0 }
    }
    fn identifier(&self) -> bool {
        self.text.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
    }
}

#[derive(Clone)]
struct Macro {
    id: usize,
    parameters: Option<Arc<Vec<String>>>,
    body: Arc<Vec<Token>>,
}

struct Conditional {
    line: usize,
    parent: bool,
    taken: bool,
    active: bool,
    saw_else: bool,
}

struct Processor {
    version: u32,
    token_limit: usize,
    macros: BTreeMap<String, Macro>,
    names: BTreeMap<String, usize>,
    work: usize,
    created_bytes: usize,
    braces: i32,
}

fn error(line: usize, message: impl std::fmt::Display) -> String {
    format!("preprocessor line {line}: {message}")
}

fn header_300(line: &str) -> bool {
    let line = line.trim_matches(|c| c == ' ' || c == '\t');
    line.strip_prefix('#').is_some_and(|rest| rest.split_ascii_whitespace().eq(["version", "300", "es"]))
}

/// Physical source lines survive normalization, splicing and comment removal.
struct Mapped {
    text: String,
    lines: Vec<usize>,
}

fn normalize(source: &str) -> Mapped {
    let bytes = source.as_bytes();
    let (mut text, mut lines, mut line, mut i) = (Vec::new(), Vec::new(), 1, 0);
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b'\r' | b'\n') {
            text.push(b'\n'); lines.push(line); line += 1;
            if bytes.get(i + 1).is_some_and(|next| matches!((b, *next), (b'\r', b'\n') | (b'\n', b'\r'))) { i += 1; }
        } else { text.push(b); lines.push(line); }
        i += 1;
    }
    Mapped { text: String::from_utf8(text).expect("normalizing newlines preserves UTF-8"), lines }
}

fn splice(source: Mapped) -> Mapped {
    // WebGL1 also requires line continuation, in addition to GLSL ES3.00.
    let bytes = source.text.as_bytes();
    let (mut text, mut lines, mut i) = (Vec::new(), Vec::new(), 0);
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'\n') { i += 2; continue; }
        text.push(bytes[i]); lines.push(source.lines[i]); i += 1;
    }
    Mapped { text: String::from_utf8(text).expect("splicing ASCII preserves UTF-8"), lines }
}

fn comments(source: Mapped) -> Result<Mapped, String> {
    let bytes = source.text.as_bytes();
    let (mut text, mut lines, mut i) = (Vec::new(), Vec::new(), 0);
    while i < bytes.len() {
        if matches!(bytes[i], b'\'' | b'"') {
            // Keep quoted preprocessing tokens intact, including in skipped
            // branches. They never enable GLSL string/character expressions.
            let quote = bytes[i];
            text.push(bytes[i]); lines.push(source.lines[i]); i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                let b = bytes[i]; text.push(b); lines.push(source.lines[i]); i += 1;
                if b == quote { break; }
                if b == b'\\' && i < bytes.len() { text.push(bytes[i]); lines.push(source.lines[i]); i += 1; }
            }
        } else if bytes.get(i..i + 2) == Some(b"//") {
            text.push(b' '); lines.push(source.lines[i]); i += 2;
            while i < bytes.len() && bytes[i] != b'\n' { i += 1; }
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            let line = source.lines[i]; text.push(b' '); lines.push(line); i += 2;
            while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" { i += 1; }
            if i + 1 >= bytes.len() { return Err(error(line, "unterminated comment")); }
            i += 2;
        } else { text.push(bytes[i]); lines.push(source.lines[i]); i += 1; }
    }
    Ok(Mapped { text: String::from_utf8(text).expect("removing comments preserves UTF-8"), lines })
}

fn tokenize(source: &Mapped, token_limit: usize) -> Result<Vec<Vec<Token>>, String> {
    let bytes = source.text.as_bytes();
    let (mut lines, mut line, mut space, mut i, mut count) = (Vec::new(), Vec::new(), false, 0, 0);
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' { lines.push(std::mem::take(&mut line)); space = true; i += 1; continue; }
        if b.is_ascii_whitespace() { space = true; i += 1; continue; }
        let start = i;
        if matches!(b, b'\'' | b'"') {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                let c = bytes[i]; i += 1;
                if c == b { break; }
                if c == b'\\' && i < bytes.len() { i += 1; }
            }
        } else if b.is_ascii_alphabetic() || b == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
            if i - start > token_limit { return Err(error(source.lines[start], "identifier exceeds token limit")); }
        } else if b.is_ascii_digit() || (b == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || (matches!(c, b'+' | b'-') && matches!(bytes[i - 1], b'e' | b'E' | b'p' | b'P')) { i += 1; }
                else { break; }
            }
        } else {
            let width = ["<<=", ">>=", "...", "&&", "||", "^^", "<<", ">>", "<=", ">=", "==", "!=", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "##"]
                .iter().find(|op| bytes[i..].starts_with(op.as_bytes())).map_or(1, |op| op.len());
            i += if b.is_ascii() { width } else { source.text[i..].chars().next().unwrap().len_utf8() };
        }
        count += 1;
        if count > MAX_TOKENS { return Err(error(source.lines[start], "preprocessing token budget exceeded")); }
        line.push(Token::new(&source.text[start..i], source.lines[start], space)); space = false;
    }
    lines.push(line);
    Ok(lines)
}

pub(crate) fn preprocess(source: &str, token_limit: usize) -> Result<String, String> {
    if source.len() > MAX_BYTES { return Err("shader source exceeds 64 KiB".into()); }
    let normalized = normalize(source);
    let first_line = normalized.text.split('\n').next().unwrap_or("");
    let strict_300 = header_300(first_line) && normalized.text.contains('\n');
    let version = if strict_300 { 300 } else { 100 };
    let lines = tokenize(&comments(splice(normalized))?, token_limit)?;
    let mut processor = Processor { version, token_limit, macros: BTreeMap::new(), names: BTreeMap::new(), work: 0, created_bytes: 0, braces: 0 };
    let (mut output, mut pending, mut stack, mut seen_any, mut version_seen) = (String::new(), Vec::new(), Vec::<Conditional>::new(), false, false);
    for line in lines {
        if line.is_empty() { continue; }
        let active = stack.last().is_none_or(|frame| frame.active);
        if line[0].text != "#" {
            seen_any = true;
            if active { pending.extend(line); }
            continue;
        }
        processor.flush(&mut pending, &mut output)?;
        let directive = line.get(1).map_or("", |token| token.text.as_str());
        let where_ = line[0].line;
        if directive == "version" {
            if seen_any || version_seen { return Err(error(where_, "#version must precede all other directives and source tokens")); }
            match line.iter().skip(2).map(|token| token.text.as_str()).collect::<Vec<_>>().as_slice() {
                ["100"] => processor.version = 100,
                ["300", "es"] if strict_300 && where_ == 1 => processor.version = 300,
                ["300", "es"] => return Err(error(where_, "#version 300 es requires the first physical line and a newline")),
                _ => return Err(error(where_, "supported versions are #version 100 and #version 300 es")),
            }
            let header = if processor.version == 300 { "#version 300 es\n" } else { "#version 100\n" };
            output.push_str(header); version_seen = true; seen_any = true;
            continue;
        }
        seen_any = true;
        match directive {
            "if" | "ifdef" | "ifndef" => {
                if stack.len() == MAX_CONDITIONAL_DEPTH { return Err(error(where_, "conditional nesting budget exceeded")); }
                let yes = if !active { false } else if directive == "if" { processor.condition(&line[2..], where_)? }
                    else { let name = one_identifier(&line[2..], where_)?; processor.defined(name) != (directive == "ifndef") };
                stack.push(Conditional { line: where_, parent: active, taken: yes, active: active && yes, saw_else: false });
            }
            "elif" => {
                let index = stack.len().checked_sub(1).ok_or_else(|| error(where_, "#elif without #if"))?;
                if stack[index].saw_else { return Err(error(where_, "#elif after #else")); }
                let yes = stack[index].parent && !stack[index].taken && processor.condition(&line[2..], where_)?;
                stack[index].active = yes; stack[index].taken |= yes;
            }
            "else" => {
                if line.len() != 2 { return Err(error(where_, "tokens after #else")); }
                let frame = stack.last_mut().ok_or_else(|| error(where_, "#else without #if"))?;
                if frame.saw_else { return Err(error(where_, "duplicate #else")); }
                frame.active = frame.parent && !frame.taken; frame.taken = true; frame.saw_else = true;
            }
            "endif" => {
                if line.len() != 2 { return Err(error(where_, "tokens after #endif")); }
                stack.pop().ok_or_else(|| error(where_, "#endif without #if"))?;
            }
            _ if !active => {},
            "" => {},
            "define" => processor.define(&line[2..], where_)?,
            "undef" => {
                let name = one_identifier(&line[2..], where_)?;
                processor.macro_name(name, where_)?; processor.macros.remove(name);
            }
            "error" => return Err(error(where_, format!("#error {}", line[2..].iter().map(|token| token.text.as_str()).collect::<Vec<_>>().join(" ")))),
            "pragma" => processor.pragma(&line[2..], where_)?,
            "extension" | "line" | "include" => return Err(error(where_, format!("#{directive} is unsupported by this bounded preprocessor"))),
            _ => return Err(error(where_, format!("unknown directive #{directive}"))),
        }
    }
    if let Some(frame) = stack.last() { return Err(error(frame.line, "unterminated conditional block")); }
    processor.flush(&mut pending, &mut output)?;
    Ok(output)
}

fn one_identifier(tokens: &[Token], line: usize) -> Result<&str, String> {
    if tokens.len() == 1 && tokens[0].identifier() { Ok(&tokens[0].text) }
    else { Err(error(line, "expected one identifier")) }
}

impl Processor {
    fn charge(&mut self, line: usize, bytes: usize) -> Result<(), String> {
        self.work = self.work.saturating_add(1);
        self.created_bytes = self.created_bytes.saturating_add(bytes);
        if self.work > MAX_WORK || self.created_bytes > MAX_CREATED_BYTES { return Err(error(line, "macro expansion work budget exceeded")); }
        Ok(())
    }
    fn builtin(&self, name: &str, line: usize) -> Option<usize> {
        match name {
            "GL_ES" | "GL_FRAGMENT_PRECISION_HIGH" => Some(1),
            "__VERSION__" => Some(self.version as usize), "__LINE__" => Some(line), "__FILE__" => Some(0), _ => None,
        }
    }
    fn defined(&self, name: &str) -> bool { self.builtin(name, 1).is_some() || self.macros.contains_key(name) }
    fn macro_name(&self, name: &str, line: usize) -> Result<(), String> {
        if self.builtin(name, line).is_some() || name == "defined" || name.starts_with("GL_") || name.starts_with("webgl_") || name.starts_with("_webgl_") || (self.version == 100 && name.contains("__")) {
            return Err(error(line, format!("reserved macro name {name}")));
        }
        if name.len() > self.token_limit { return Err(error(line, "macro name exceeds token limit")); }
        Ok(())
    }
    fn define(&mut self, tokens: &[Token], line: usize) -> Result<(), String> {
        let name = tokens.first().filter(|token| token.identifier()).ok_or_else(|| error(line, "#define requires an identifier"))?;
        self.macro_name(&name.text, line)?;
        let (mut pos, mut parameters) = (1, None);
        if tokens.get(pos).is_some_and(|token| token.text == "(" && !token.space) {
            pos += 1; let mut names = Vec::new();
            if tokens.get(pos).is_some_and(|token| token.text == ")") { pos += 1; }
            else { loop {
                let parameter = tokens.get(pos).filter(|token| token.identifier()).ok_or_else(|| error(line, "expected macro parameter; variadic macros are unsupported"))?;
                if names.contains(&parameter.text) { return Err(error(line, "duplicate macro parameter")); }
                names.push(parameter.text.clone()); pos += 1;
                if names.len() > MAX_ARGUMENTS { return Err(error(line, "macro parameter budget exceeded")); }
                match tokens.get(pos).map(|token| token.text.as_str()) {
                    Some(")") => { pos += 1; break; }, Some(",") => pos += 1,
                    _ => return Err(error(line, "unterminated macro parameter list; variadics are unsupported")),
                }
            } }
            parameters = Some(Arc::new(names));
        }
        let body = &tokens[pos..];
        if body.iter().any(|token| matches!(token.text.as_str(), "#" | "##" | "...")) { return Err(error(line, "stringification, token pasting and variadic macros are unsupported")); }
        if let Some(previous) = self.macros.get(&name.text) {
            let identical = previous.parameters == parameters && previous.body.len() == body.len()
                && previous.body.iter().zip(body).enumerate().all(|(index, (a, b))| a.text == b.text && (index == 0 || a.space == b.space));
            if identical { return Ok(()); }
            return Err(error(line, format!("incompatible redefinition of {}", name.text)));
        }
        let id = if let Some(id) = self.names.get(&name.text) { *id } else {
            if self.names.len() == MAX_MACROS { return Err(error(line, "macro name budget exceeded")); }
            let id = self.names.len(); self.names.insert(name.text.clone(), id); id
        };
        self.macros.insert(name.text.clone(), Macro { id, parameters, body: Arc::new(body.to_vec()) });
        Ok(())
    }
    fn pragma(&self, tokens: &[Token], line: usize) -> Result<(), String> {
        if tokens.first().is_some_and(|token| matches!(token.text.as_str(), "optimize" | "debug")) {
            if self.braces != 0 { return Err(error(line, "optimization/debug pragmas must be outside function definitions")); }
            if tokens.len() != 4 || tokens[1].text != "(" || !matches!(tokens[2].text.as_str(), "on" | "off") || tokens[3].text != ")" { return Err(error(line, "malformed optimization/debug pragma")); }
        }
        // Unknown pragmas are ignored by the GLSL rule; no extension is enabled.
        Ok(())
    }
    fn flush(&mut self, pending: &mut Vec<Token>, output: &mut String) -> Result<(), String> {
        if pending.is_empty() { return Ok(()); }
        let tokens = self.expand(std::mem::take(pending), false, 0)?;
        for token in tokens {
            if token.text == "#" { return Err(error(token.line, "# must start a directive")); }
            match token.text.as_str() { "{" => self.braces += 1, "}" => self.braces -= 1, _ => {} }
            if output.len() + token.text.len() + 2 > MAX_BYTES { return Err(error(token.line, "expanded shader exceeds 64 KiB")); }
            output.push_str(&token.text); output.push(' ');
        }
        if !output.ends_with('\n') { output.push('\n'); }
        Ok(())
    }
    fn condition(&mut self, tokens: &[Token], line: usize) -> Result<bool, String> {
        let tokens = self.expand(tokens.to_vec(), true, 0)?;
        let mut expression = ConditionParser { tokens: &tokens, pos: 0, depth: 0, line };
        let value = expression.expression(1, true)?;
        if expression.pos != tokens.len() { return Err(error(tokens[expression.pos].line, "unexpected token in #if expression")); }
        Ok(value != 0)
    }

    fn expand(&mut self, tokens: Vec<Token>, conditional: bool, depth: usize) -> Result<Vec<Token>, String> {
        if depth > MAX_EXPANSION_DEPTH { return Err(error(tokens.first().map_or(1, |token| token.line), "macro argument expansion nesting budget exceeded")); }
        let mut queue: VecDeque<_> = tokens.into();
        let (mut output, mut output_bytes) = (Vec::new(), 0usize);
        while let Some(token) = queue.pop_front() {
            self.charge(token.line, token.text.len() + 1)?;
            if conditional && token.text == "defined" {
                if token.depth != 0 || !token.hidden.is_empty() { return Err(error(token.line, "macro-generated defined is forbidden by WebGL")); }
                let parenthesized = queue.front().is_some_and(|next| next.text == "(");
                if parenthesized { queue.pop_front(); }
                let name = queue.pop_front().filter(|name| name.identifier()).ok_or_else(|| error(token.line, "defined requires an identifier"))?;
                if parenthesized && !queue.pop_front().is_some_and(|close| close.text == ")") { return Err(error(token.line, "defined requires a closing parenthesis")); }
                let value = if self.defined(&name.text) { "1" } else { "0" };
                output.push(Token::new(value, token.line, token.space)); output_bytes += 2;
                if output.len() > MAX_TOKENS || output_bytes > MAX_BYTES { return Err(error(token.line, "expanded token/output budget exceeded")); }
                continue;
            }
            if let Some(value) = self.builtin(&token.text, token.line) {
                let value = value.to_string(); output_bytes += value.len() + 1;
                output.push(Token::new(&value, token.line, token.space));
            } else if let Some(definition) = self.macros.get(&token.text).cloned().filter(|definition| !token.hidden.contains(&definition.id)) {
                if definition.parameters.is_some() && !queue.front().is_some_and(|next| next.text == "(") {
                    output_bytes += token.text.len() + 1; output.push(token);
                    if output.len() > MAX_TOKENS || output_bytes > MAX_BYTES { return Err(error(output.last().unwrap().line, "expanded token/output budget exceeded")); }
                    continue;
                }
                if token.depth >= MAX_EXPANSION_DEPTH { return Err(error(token.line, "macro expansion depth budget exceeded")); }
                let mut hidden = token.hidden.clone();
                let mut arguments = Vec::new();
                if let Some(parameters) = &definition.parameters {
                    queue.pop_front();
                    let mut current = Vec::new(); let mut nesting = 0;
                    let close = loop {
                        let next = queue.pop_front().ok_or_else(|| error(token.line, "unterminated macro invocation"))?;
                        self.charge(next.line, next.text.len() + 1)?;
                        match next.text.as_str() {
                            "(" => {
                                nesting += 1;
                                if nesting > MAX_CONDITIONAL_DEPTH { return Err(error(next.line, "macro argument parenthesis nesting budget exceeded")); }
                            }
                            ")" if nesting == 0 => { arguments.push(current); break next; }
                            ")" => nesting -= 1,
                            "," if nesting == 0 => {
                                arguments.push(std::mem::take(&mut current));
                                if arguments.len() >= MAX_ARGUMENTS { return Err(error(next.line, "macro argument budget exceeded")); }
                                continue;
                            }
                            _ => {},
                        }
                        current.push(next);
                    };
                    if parameters.is_empty() && arguments.len() == 1 && arguments[0].is_empty() { arguments.clear(); }
                    if parameters.len() != arguments.len() { return Err(error(token.line, format!("macro {} expects {} arguments, got {}", token.text, parameters.len(), arguments.len()))); }
                    // The closing token contributes to the function invocation's
                    // hide set; intersecting avoids over-disabling later rescans.
                    hidden = hidden.intersection(&close.hidden).copied().collect();
                }
                hidden.insert(definition.id);
                let mut cached: Vec<Option<Vec<Token>>> = vec![None; arguments.len()];
                let mut replacement = Vec::new();
                for item in definition.body.iter() {
                    let parameter = definition.parameters.as_ref().and_then(|parameters| parameters.iter().position(|name| name == &item.text));
                    if let Some(index) = parameter {
                        // Unused parameters are never prescanned. This permits
                        // IGNORE(bad_macro(arity)) without expanding discarded code.
                        if cached[index].is_none() { cached[index] = Some(self.expand(arguments[index].clone(), conditional, depth + 1)?); }
                        for argument in cached[index].as_ref().unwrap() {
                            let mut argument = argument.clone();
                            argument.hidden.extend(hidden.iter().copied()); argument.depth = argument.depth.max(token.depth + 1);
                            self.charge(argument.line, argument.text.len() + 1)?; replacement.push(argument);
                        }
                    } else {
                        let mut item = item.clone(); item.line = token.line;
                        item.hidden.extend(hidden.iter().copied()); item.depth = token.depth + 1;
                        self.charge(item.line, item.text.len() + 1)?; replacement.push(item);
                    }
                }
                if queue.len() + replacement.len() > MAX_TOKENS { return Err(error(token.line, "macro expansion token budget exceeded")); }
                for item in replacement.into_iter().rev() { queue.push_front(item); }
            } else {
                output_bytes += token.text.len() + 1; output.push(token);
            }
            if output.len() > MAX_TOKENS || output_bytes > MAX_BYTES { return Err(error(output.last().map_or(1, |token| token.line), "expanded token/output budget exceeded")); }
        }
        Ok(output)
    }
}

struct ConditionParser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
    line: usize,
}

fn precedence(operator: &str) -> Option<u8> {
    Some(match operator {
        "||" => 1, "&&" => 2, "|" => 3, "^" => 4, "&" => 5,
        "==" | "!=" => 6, "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8, "+" | "-" => 9, "*" | "/" | "%" => 10,
        _ => return None,
    })
}

fn integer(token: &Token) -> Result<i32, String> {
    let text = token.text.as_str();
    if text.ends_with(['u', 'U']) { return Err(error(token.line, "unsigned #if arithmetic is unsupported")); }
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) { (hex, 16) }
        else if text.len() > 1 && text.starts_with('0') { (&text[1..], 8) } else { (text, 10) };
    i32::from_str_radix(digits, radix).map_err(|_| error(token.line, "#if requires a signed 32-bit integer literal"))
}

impl ConditionParser<'_> {
    fn expression(&mut self, minimum: u8, evaluate: bool) -> Result<i32, String> {
        if self.depth == MAX_CONDITIONAL_DEPTH { return Err(error(self.line, "#if expression nesting budget exceeded")); }
        self.depth += 1;
        let result = self.expression_inner(minimum, evaluate);
        self.depth -= 1;
        result
    }
    fn expression_inner(&mut self, minimum: u8, evaluate: bool) -> Result<i32, String> {
        let token = self.tokens.get(self.pos).cloned().ok_or_else(|| error(self.line, "missing operand in #if expression"))?;
        self.pos += 1;
        let mut left = match token.text.as_str() {
            "+" | "-" | "!" | "~" => {
                let value = self.expression(11, evaluate)?;
                if !evaluate { 0 } else { match token.text.as_str() {
                    "+" => value, "-" => value.checked_neg().ok_or_else(|| error(token.line, "signed #if negation overflow"))?,
                    "!" => i32::from(value == 0), "~" => !value, _ => unreachable!(),
                } }
            }
            "(" => {
                let value = self.expression(1, evaluate)?;
                if !self.tokens.get(self.pos).is_some_and(|token| token.text == ")") { return Err(error(token.line, "unclosed #if parenthesis")); }
                self.pos += 1; value
            }
            _ if token.identifier() => {
                if evaluate { return Err(error(token.line, format!("undefined #if identifier {}", token.text))); }
                0
            }
            _ => integer(&token)?,
        };
        while let Some(operator) = self.tokens.get(self.pos).cloned() {
            let Some(level) = precedence(&operator.text) else { break };
            if level < minimum { break; }
            self.pos += 1;
            let right_evaluated = evaluate && match operator.text.as_str() { "&&" => left != 0, "||" => left == 0, _ => true };
            let right = self.expression(level + 1, right_evaluated)?;
            left = if evaluate { apply(&operator.text, left, right, operator.line)? } else { 0 };
        }
        Ok(left)
    }
}

fn apply(operator: &str, left: i32, right: i32, line: usize) -> Result<i32, String> {
    let overflow = || error(line, "invalid or overflowing signed #if arithmetic");
    Ok(match operator {
        "+" => left.checked_add(right).ok_or_else(overflow)?, "-" => left.checked_sub(right).ok_or_else(overflow)?,
        "*" => left.checked_mul(right).ok_or_else(overflow)?, "/" => left.checked_div(right).ok_or_else(overflow)?,
        "%" => left.checked_rem(right).ok_or_else(overflow)?,
        "<<" | ">>" => {
            if !(0..32).contains(&right) { return Err(error(line, "#if shift count must be 0..31")); }
            if operator == "<<" { left.wrapping_shl(right as u32) } else { left >> right }
        }
        "<" => i32::from(left < right), ">" => i32::from(left > right), "<=" => i32::from(left <= right), ">=" => i32::from(left >= right),
        "==" => i32::from(left == right), "!=" => i32::from(left != right),
        "&" => left & right, "|" => left | right, "^" => left ^ right,
        "&&" => i32::from(left != 0 && right != 0), "||" => i32::from(left != 0 || right != 0),
        _ => return Err(error(line, "unsupported #if operator")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(source: &str) -> Vec<String> {
        preprocess(source, 1024).unwrap().split_whitespace().map(str::to_owned).collect()
    }
    fn rejects(source: &str, detail: &str) {
        let message = preprocess(source, 1024).unwrap_err();
        assert!(message.contains(detail), "{source:?}: {message}");
    }

    #[test]
    fn substitution_uses_tokens_and_discards_comments_and_inactive_unicode() {
        assert_eq!(words("#define A 0.25\n#define AB 0.75\nAB A /* A */ // AB\n"), ["0.75", "0.25"]);
        assert_eq!(words("#define A 0.25\n#if 0\n☃ \\\n\"/* A */\"\n#line 42 \"http://unused\"\n#endif\nA"), ["0.25"]);
        // Quoted preprocessing tokens are not expanded into new identifiers.
        assert_eq!(words("#define A 1\n\"A\" A"), ["\"A\"", "1"]);
        rejects("/* never closed", "unterminated comment");
        rejects("/*/", "unterminated comment");
        assert_eq!(words("/***/ 0.5 // comment at EOF"), ["0.5"]);
    }

    #[test]
    fn function_arguments_are_prescanned_and_replacement_is_rescanned() {
        let source = "#define A 0.25\n#define F(x,y) mix(x,y,0.5)\n#define ALIAS F\nALIAS(A,F(A,0.75))";
        assert_eq!(words(source), ["mix", "(", "0.25", ",", "mix", "(", "0.25", ",", "0.75", ",", "0.5", ")", ",", "0.5", ")"]);
        assert_eq!(words("#define BAD(x) x\n#define IGNORE(x) 0.5\nIGNORE(BAD(1,2))"), ["0.5"]);
        assert_eq!(words("#define EMPTY()\n#define ID(x) x\nEMPTY() ID() 0.5"), ["0.5"]);
        assert_eq!(words("#define WRAP (0.5)\nWRAP"), ["(", "0.5", ")"]);
        assert_eq!(words("#define F/**/(x)\nF"), ["(", "x", ")"]);
        assert_eq!(words("#define F(x,y) ((x)+(y))\nF\n(0.25,\n0.5)"), ["(", "(", "0.25", ")", "+", "(", "0.5", ")", ")"]);
        rejects("#define F(x) x\nF(1,2)", "expects 1 arguments");
        rejects("#define F(x) x\nF(1", "unterminated macro invocation");
    }

    #[test]
    fn hide_sets_suppress_self_reference_without_disabling_unrelated_calls() {
        assert_eq!(words("#define SELF SELF\nSELF"), ["SELF"]);
        assert_eq!(words("#define A B\n#define B A\nA"), ["A"]);
        assert_eq!(words("#define F(x) F(x)\nF(0.5)"), ["F", "(", "0.5", ")"]);
        assert_eq!(words("#define F(x) x\n#define G F\nF(G)(0.5)"), ["F", "(", "0.5", ")"]);
        // Khronos CTS macro-expansion-tricky.html, pinned in docs/glsl-preprocessor.md:
        // a discarded argument contains an incomplete, recursively named invocation.
        assert!(words("#define m(a)\n#define a m((a)\na)").is_empty());
    }

    #[test]
    fn redefinition_undefinition_and_macro_name_rules_are_explicit() {
        assert_eq!(words("#define A 0.25\n#define A 0.25\n#undef A\n#define A 0.75\nA"), ["0.75"]);
        rejects("#define A 1\n#define A 2", "redefinition");
        rejects("#define F(x) x\n#define F(y) y", "redefinition");
        rejects("#define F(x,x) x", "duplicate");
        for name in ["__LINE__", "__FILE__", "__VERSION__", "GL_ES", "GL_FRAGMENT_PRECISION_HIGH", "GL_private", "defined", "webgl_private", "_webgl_private", "a__b"] {
            rejects(&format!("#define {name} 1\n"), "reserved");
            rejects(&format!("#undef {name}\n"), "reserved");
        }
        assert_eq!(words("#version 300 es\n#define a__b 1\na__b"), ["#version", "300", "es", "1"]);
    }

    #[test]
    fn physical_lines_survive_comments_normalization_and_nonrecursive_splicing() {
        assert_eq!(words("#define WHERE __LINE__\r\n/* comment\r\nblock */\r\nWHERE __LINE__ __FILE__ __VERSION__ GL_ES GL_FRAGMENT_PRECISION_HIGH"), ["4", "4", "0", "100", "1", "1"]);
        assert_eq!(words("#define VA\\\nL 0.25\nVAL __LINE__"), ["0.25", "3"]);
        assert_eq!(words("#version 300 es\n#define VA\\\nL 0.25\nVAL __LINE__ __VERSION__"), ["#version", "300", "es", "0.25", "4", "300"]);
        assert_eq!(words("// ignored\\\n#define BAD 1\n#ifdef BAD\n#error continued comment expanded\n#endif\n__LINE__"), ["6"]);
        assert_eq!(words("#define X /* comment\ncontinuation */ 0.5\nX __LINE__"), ["0.5", "3"]);
        assert_eq!(words("0.5\\\\\n\n"), ["0.5", "\\"]);
    }

    #[test]
    fn conditionals_choose_one_branch_with_gles_short_circuit_rules() {
        let source = "#define COUNT 3\n#if defined(COUNT) && COUNT==3\nselected\n#elif UNDEFINED\nwrong\n#else\nwrong\n#endif\n#ifndef MISSING\nsecond\n#endif\n#if 0 && MISSING\nwrong\n#elif 1 || MISSING\nthird\n#endif";
        assert_eq!(words(source), ["selected", "second", "third"]);
        assert_eq!(words("#if 0 && (1/0)\nwrong\n#else\nselected\n#endif"), ["selected"]);
        assert_eq!(words("#if 1 || (1/0)\nselected\n#endif"), ["selected"]);
        assert_eq!(words("#if 0\n#if undefined / invalid\n#error ignored\n#endif\n#else\nselected\n#endif"), ["selected"]);
        for condition in ["MISSING", "0 || MISSING", "1 && MISSING"] { rejects(&format!("#if {condition}\n#endif"), "undefined"); }
        rejects("#define D defined\n#if D(X)\n#endif", "macro-generated defined");
        rejects("#define D defined\n#if 0 && D(X)\n#endif", "macro-generated defined");
        rejects("#define D(x) defined(x)\n#if D(X)\n#endif", "macro-generated defined");
        rejects("#define EMPTY\n#if EMPTY\n#endif", "missing operand");
    }

    #[test]
    fn conditional_integer_precedence_and_deferred_cases_are_checked() {
        let condition = "(0x10+010*2)==32 && (1<<4)==16 && (-8>>2)==-2 && ~0==-1 && 7%4==3 && !0 && (6&3)==2 && (5|2)==7 && (5^1)==4 && 9/2==4";
        assert_eq!(words(&format!("#if {condition}\nselected\n#else\n#error arithmetic\n#endif")), ["selected"]);
        for condition in ["1/0", "1%0", "2147483647+1", "1<<32", "1<<-1", "0.5", "2147483648", "1u", "1 ? 2 : 3"] {
            assert!(preprocess(&format!("#if {condition}\n#endif"), 1024).is_err(), "{condition}");
        }
    }

    #[test]
    fn directives_versions_and_unsupported_operations_never_vanish_silently() {
        for source in ["#else", "#elif 1", "#endif", "#if 1", "#if\n#endif", "#if 0\n#else\n#else\n#endif", "#if 0\n#else\n#elif 1\n#endif", "#if 0\n#endif junk", "#if 0\n#else junk\n#endif", "#if defined()\n#endif"] {
            assert!(preprocess(source, 1024).is_err(), "{source}");
        }
        for directive in ["#line 2", "#extension all : disable", "#include <x>", "#unknown"] { assert!(preprocess(directive, 1024).is_err(), "{directive}"); }
        for source in ["#define F(x) #x", "#define F(x,y) x##y", "#define F(...) 1"] { assert!(preprocess(source, 1024).is_err(), "{source}"); }
        rejects("#error expected marker", "expected marker");
        assert_eq!(words("#pragma unknown_tokens\n#pragma optimize(off)\n#pragma debug(on)\n0.5"), ["0.5"]);
        rejects("void f(){\n#pragma optimize(off)\n}", "outside function");
        rejects("#pragma debug(maybe)", "malformed");
        assert_eq!(words("/* comment */\n# version 100\n0.5"), ["#version", "100", "0.5"]);
        for source in ["\n#version 300 es\n", "/* comment */#version 300 es\n", "#version 300 es // comment\n", "#version 300 es", "#define V 300\n#version V es\n", "0.5\n#version 100\n", "#version 100\n#version 100\n", "#version 310 es\n"] {
            assert!(preprocess(source, 1024).is_err(), "{source:?}");
        }
    }

    #[test]
    fn global_expansion_and_structural_budgets_reject_resource_growth() {
        let mut chain = String::new();
        for index in 0..34 { chain.push_str(&format!("#define M{index} M{}\n", index + 1)); }
        chain.push_str("#define M34 0.5\nM0"); rejects(&chain, "depth budget");
        let mut duplicate = "0.5".to_string();
        for _ in 0..16 { duplicate = format!("D({duplicate})"); }
        rejects(&format!("#define D(x) x x\n{duplicate}"), "budget");
        let macros = (0..257).map(|i| format!("#define M{i} 1\n")).collect::<String>(); rejects(&macros, "macro name budget");
        rejects(&format!("{}{}", "#if 1\n".repeat(65), "#endif\n".repeat(65)), "conditional nesting budget");
        let parameters = (0..33).map(|i| format!("x{i}")).collect::<Vec<_>>().join(","); rejects(&format!("#define F({parameters}) 0.5\n"), "parameter budget");
        let long = "a".repeat(250); rejects(&format!("#define A {long}\n{}", "A ".repeat(300)), "budget");
        rejects(&" ".repeat(MAX_BYTES + 1), "64 KiB");
    }
}
