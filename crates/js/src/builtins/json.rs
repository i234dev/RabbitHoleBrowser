//! JSON.parse / JSON.stringify.

use super::*;
use crate::value::V;
use crate::realm::SYM_TO_STRING_TAG;

pub fn register(realm: &mut Realm) {
    let j = realm.new_object();
    realm.define_global("JSON", Value::Object(j));
    realm.define_method(j, "parse", 2, parse);
    realm.define_method(j, "stringify", 3, stringify);
    realm.define_method(j, "rawJSON", 1, raw_json);
    realm.define_method(j, "isRawJSON", 1, is_raw_json);
    // [[IsRawJSON]] is an own property under a private symbol.
    let marker = realm.new_private_symbol(JsStr::new("IsRawJSON"));
    let holder = realm.new_object_with_proto(None);
    realm.define_property(holder, "sym", Value::Symbol(marker), 0);
    realm.intrinsics.others.insert("RawJSONMarker", holder);
    realm.define_property(j, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("JSON"), CONFIGURABLE);
}

fn raw_json_key(realm: &Realm) -> Option<PropertyKey> {
    let holder = *realm.intrinsics.others.get("RawJSONMarker")?;
    match realm.obj(holder).props.get(&PropertyKey::str("sym")).map(|p| p.value.v()) {
        Some(V::Symbol(s)) => Some(PropertyKey::Symbol(s)),
        _ => None,
    }
}

fn is_raw_json_object(realm: &Realm, id: ObjectId) -> bool {
    raw_json_key(realm).is_some_and(|k| realm.obj(id).props.contains(&k))
}

/// JSON.rawJSON(text): a frozen null-prototype object whose `rawJSON` is
/// the text of a JSON primitive, serialized verbatim by JSON.stringify.
fn raw_json(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = realm.to_string(arg(args, 0))?;
    let u = text.as_units();
    let ws = |c: u16| matches!(c, 0x09 | 0x0a | 0x0d | 0x20);
    if u.is_empty() || ws(u[0]) || ws(u[u.len() - 1]) {
        return Err(realm.new_error("SyntaxError", "Invalid value for JSON.rawJSON"));
    }
    if matches!(u[0], 0x7b | 0x5b) {
        return Err(realm.new_error("SyntaxError", "JSON.rawJSON cannot create objects or arrays"));
    }
    let mut p = Parser { s: u, pos: 0 };
    p.value(realm, 0)?;
    p.ws();
    if p.pos < p.s.len() {
        return p.err(realm, "Unexpected non-whitespace character after JSON");
    }
    let obj = realm.new_object_with_proto(None);
    realm.define_property(obj, "rawJSON", Value::String(text), ENUMERABLE);
    if let Some(k) = raw_json_key(realm) {
        realm.define_property(obj, k, Value::Bool(true), 0);
    }
    realm.obj_mut(obj).extensible = false;
    Ok(Value::Object(obj))
}

fn is_raw_json(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(match arg(args, 0).v() {
        V::Object(id) => is_raw_json_object(realm, id),
        _ => false,
    }))
}

/// JSON Parse Record (json-parse-with-source): the parsed value with the
/// source text of primitives and the records of members.
enum ParseRecord {
    Primitive(Value, (usize, usize)),
    Array(Value, Vec<ParseRecord>),
    Object(Value, Vec<(JsStr, ParseRecord)>),
}

impl ParseRecord {
    fn value(&self) -> &Value {
        match self {
            ParseRecord::Primitive(v, _) | ParseRecord::Array(v, _) | ParseRecord::Object(v, _) => v,
        }
    }
}

struct Parser<'a> {
    s: &'a [u16],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u16> {
        self.s.get(self.pos).copied()
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.pos += 1;
        }
    }
    fn err<T>(&self, realm: &mut Realm, msg: &str) -> JsResult<T> {
        Err(realm.new_error("SyntaxError", &format!("{} in JSON at position {}", msg, self.pos)))
    }
    fn expect(&mut self, realm: &mut Realm, c: u8) -> JsResult<()> {
        if self.peek() == Some(c as u16) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(realm, &format!("Expected '{}'", c as char))
        }
    }
    fn value(&mut self, realm: &mut Realm, depth: usize) -> JsResult<Value> {
        // Deep nesting is fine (browsers parse thousands of levels); only
        // the native stack bounds it.
        if crate::stack::exhausted() {
            return realm.range_error("Maximum call stack size exceeded");
        }
        self.ws();
        match self.peek() {
            None => self.err(realm, "Unexpected end of JSON input"),
            Some(c) if c == b'{' as u16 => {
                self.pos += 1;
                let obj = realm.new_object();
                self.ws();
                if self.peek() == Some(b'}' as u16) {
                    self.pos += 1;
                    return Ok(Value::Object(obj));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"' as u16) {
                        return self.err(realm, "Expected property name");
                    }
                    let key = self.key(realm)?;
                    self.ws();
                    self.expect(realm, b':')?;
                    let v = self.value(realm, depth + 1)?;
                    // A fresh ordinary object: CreateDataProperty is a plain
                    // insert (a repeated key replaces the earlier value).
                    realm.obj_mut(obj).props.insert(PropertyKey::String(key), Property::data(v, ALL));
                    self.ws();
                    match self.peek() {
                        Some(c) if c == b',' as u16 => self.pos += 1,
                        Some(c) if c == b'}' as u16 => {
                            self.pos += 1;
                            return Ok(Value::Object(obj));
                        }
                        _ => return self.err(realm, "Expected ',' or '}'"),
                    }
                }
            }
            Some(c) if c == b'[' as u16 => {
                self.pos += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']' as u16) {
                    self.pos += 1;
                    return Ok(realm.new_array_value(items));
                }
                loop {
                    let v = self.value(realm, depth + 1)?;
                    items.push(v);
                    self.ws();
                    match self.peek() {
                        Some(c) if c == b',' as u16 => self.pos += 1,
                        Some(c) if c == b']' as u16 => {
                            self.pos += 1;
                            return Ok(realm.new_array_value(items));
                        }
                        _ => return self.err(realm, "Expected ',' or ']'"),
                    }
                }
            }
            Some(c) if c == b'"' as u16 => Ok(Value::String(self.string(realm)?)),
            Some(c) if c == b't' as u16 => self.literal(realm, "true", Value::Bool(true)),
            Some(c) if c == b'f' as u16 => self.literal(realm, "false", Value::Bool(false)),
            Some(c) if c == b'n' as u16 => self.literal(realm, "null", Value::Null),
            Some(c) if c == b'-' as u16 || (b'0' as u16..=b'9' as u16).contains(&c) => self.number(realm),
            Some(_) => self.err(realm, "Unexpected token"),
        }
    }
    /// Like `value`, also recording the source text of every primitive.
    fn value_record(&mut self, realm: &mut Realm) -> JsResult<ParseRecord> {
        if crate::stack::exhausted() {
            return realm.range_error("Maximum call stack size exceeded");
        }
        self.ws();
        match self.peek() {
            Some(c) if c == b'{' as u16 => {
                self.pos += 1;
                let obj = realm.new_object();
                let mut entries: Vec<(JsStr, ParseRecord)> = Vec::new();
                self.ws();
                if self.peek() == Some(b'}' as u16) {
                    self.pos += 1;
                    return Ok(ParseRecord::Object(Value::Object(obj), entries));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"' as u16) {
                        return self.err(realm, "Expected property name");
                    }
                    let key = self.key(realm)?;
                    self.ws();
                    self.expect(realm, b':')?;
                    let r = self.value_record(realm)?;
                    realm.obj_mut(obj).props.insert(PropertyKey::String(key.clone()), Property::data(r.value().clone(), ALL));
                    entries.retain(|(k, _)| *k != key);
                    entries.push((key, r));
                    self.ws();
                    match self.peek() {
                        Some(c) if c == b',' as u16 => self.pos += 1,
                        Some(c) if c == b'}' as u16 => {
                            self.pos += 1;
                            return Ok(ParseRecord::Object(Value::Object(obj), entries));
                        }
                        _ => return self.err(realm, "Expected ',' or '}'"),
                    }
                }
            }
            Some(c) if c == b'[' as u16 => {
                self.pos += 1;
                let mut recs = Vec::new();
                self.ws();
                if self.peek() != Some(b']' as u16) {
                    loop {
                        recs.push(self.value_record(realm)?);
                        self.ws();
                        match self.peek() {
                            Some(c) if c == b',' as u16 => self.pos += 1,
                            Some(c) if c == b']' as u16 => break,
                            _ => return self.err(realm, "Expected ',' or ']'"),
                        }
                    }
                }
                self.pos += 1;
                let items = recs.iter().map(|r| r.value().clone()).collect();
                Ok(ParseRecord::Array(realm.new_array_value(items), recs))
            }
            _ => {
                let start = self.pos;
                let v = self.value(realm, 0)?;
                Ok(ParseRecord::Primitive(v, (start, self.pos)))
            }
        }
    }
    fn literal(&mut self, realm: &mut Realm, word: &str, v: Value) -> JsResult<Value> {
        for b in word.bytes() {
            if self.peek() != Some(b as u16) {
                return self.err(realm, "Unexpected token");
            }
            self.pos += 1;
        }
        Ok(v)
    }
    fn number(&mut self, realm: &mut Realm) -> JsResult<Value> {
        let start = self.pos;
        if self.peek() == Some(b'-' as u16) {
            self.pos += 1;
        }
        match self.peek() {
            Some(c) if c == b'0' as u16 => self.pos += 1,
            Some(c) if (b'1' as u16..=b'9' as u16).contains(&c) => {
                while matches!(self.peek(), Some(c) if (b'0' as u16..=b'9' as u16).contains(&c)) {
                    self.pos += 1;
                }
            }
            _ => return self.err(realm, "No number after minus sign"),
        }
        if self.peek() == Some(b'.' as u16) {
            self.pos += 1;
            let ds = self.pos;
            while matches!(self.peek(), Some(c) if (b'0' as u16..=b'9' as u16).contains(&c)) {
                self.pos += 1;
            }
            if ds == self.pos {
                return self.err(realm, "Unterminated fractional number");
            }
        }
        if matches!(self.peek(), Some(c) if c == b'e' as u16 || c == b'E' as u16) {
            self.pos += 1;
            if matches!(self.peek(), Some(c) if c == b'+' as u16 || c == b'-' as u16) {
                self.pos += 1;
            }
            let ds = self.pos;
            while matches!(self.peek(), Some(c) if (b'0' as u16..=b'9' as u16).contains(&c)) {
                self.pos += 1;
            }
            if ds == self.pos {
                return self.err(realm, "Exponent part is missing a number");
            }
        }
        let digits = &self.s[start..self.pos];
        // Short integers: exact without going through the float parser.
        if digits.len() <= 15 && digits.iter().all(|&c| (b'0' as u16..=b'9' as u16).contains(&c) || c == b'-' as u16) {
            let neg = digits[0] == b'-' as u16;
            let mut n: i64 = 0;
            for &c in &digits[neg as usize..] {
                n = n * 10 + (c - b'0' as u16) as i64;
            }
            return Ok(Value::Number(if neg { -(n as f64) } else { n as f64 }));
        }
        let text = String::from_utf16_lossy(digits);
        Ok(Value::Number(text.parse::<f64>().unwrap_or(f64::NAN)))
    }

    /// An object key: short ASCII keys without escapes are interned (shared
    /// by every object parsed with the same layout).
    fn key(&mut self, realm: &mut Realm) -> JsResult<JsStr> {
        let start = self.pos + 1;
        let mut end = start;
        while end < self.s.len() && end - start <= 32 {
            let c = self.s[end];
            if c == b'"' as u16 {
                let units = &self.s[start..end];
                let ascii: String = units.iter().map(|&u| u as u8 as char).collect();
                self.pos = end + 1;
                return Ok(JsStr::intern(&ascii));
            }
            if c >= 0x80 || c < 0x20 || c == b'\\' as u16 {
                break;
            }
            end += 1;
        }
        self.string(realm)
    }
    fn string(&mut self, realm: &mut Realm) -> JsResult<JsStr> {
        self.pos += 1; // opening quote
        let mut out: Vec<u16> = Vec::new();
        loop {
            match self.peek() {
                None => return self.err(realm, "Unterminated string"),
                Some(c) if c == b'"' as u16 => {
                    self.pos += 1;
                    return Ok(JsStr::from_units(out));
                }
                Some(c) if c == b'\\' as u16 => {
                    self.pos += 1;
                    let e = match self.peek() {
                        Some(e) => e,
                        None => return self.err(realm, "Unterminated string"),
                    };
                    self.pos += 1;
                    match e as u8 {
                        b'"' => out.push(b'"' as u16),
                        b'\\' => out.push(b'\\' as u16),
                        b'/' => out.push(b'/' as u16),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(10),
                        b'r' => out.push(13),
                        b't' => out.push(9),
                        b'u' => {
                            if self.pos + 4 > self.s.len() {
                                return self.err(realm, "Bad Unicode escape");
                            }
                            let hex = String::from_utf16_lossy(&self.s[self.pos..self.pos + 4]);
                            match u16::from_str_radix(&hex, 16) {
                                Ok(u) => out.push(u),
                                Err(_) => return self.err(realm, "Bad Unicode escape"),
                            }
                            self.pos += 4;
                        }
                        _ => return self.err(realm, "Bad escaped character"),
                    }
                }
                Some(c) if c < 0x20 => return self.err(realm, "Bad control character in string literal"),
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }
}

fn parse(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let text = realm.to_string(arg(args, 0))?;
    let reviver = arg(args, 1);
    let mut p = Parser { s: text.as_units(), pos: 0 };
    if !realm.is_callable(&reviver) {
        let v = p.value(realm, 0)?;
        p.ws();
        if p.pos < p.s.len() {
            return p.err(realm, "Unexpected non-whitespace character after JSON");
        }
        return Ok(v);
    }
    let rec = p.value_record(realm)?;
    p.ws();
    if p.pos < p.s.len() {
        return p.err(realm, "Unexpected non-whitespace character after JSON");
    }
    let root = realm.new_object();
    realm.define_property(root, "", rec.value().clone(), ALL);
    internalize(realm, root, PropertyKey::str(""), &reviver, Some(&rec), text.as_units())
}

/// InternalizeJSONProperty, passing the reviver a context with the source
/// text of primitives that are still the parsed values.
fn internalize(realm: &mut Realm, holder: ObjectId, key: PropertyKey, reviver: &Value, rec: Option<&ParseRecord>, src: &[u16]) -> JsResult<Value> {
    let val = realm.get(Value::Object(holder), key.clone())?;
    let context = realm.new_object();
    let rec = rec.filter(|r| realm.same_value(r.value(), &val));
    if let Some(ParseRecord::Primitive(_, (a, b))) = rec {
        realm.define_property(context, "source", Value::String(JsStr::from_units(src[*a..*b].to_vec())), ALL);
    }
    if let V::Object(id) = val.v() {
        if array::is_array_checked(realm, &val)? {
            let elems: &[ParseRecord] = match rec {
                Some(ParseRecord::Array(_, e)) => e,
                _ => &[],
            };
            let len = realm.length_of(val.clone())?;
            for i in 0..len {
                let k = array::key_of(i);
                let nv = internalize(realm, id, k.clone(), reviver, elems.get(i), src)?;
                if nv.is_undefined() {
                    realm.delete_property(id, &k)?;
                } else {
                    realm.define_own(id, k, Property::data(nv, ALL))?;
                }
            }
        } else {
            let entries: &[(JsStr, ParseRecord)] = match rec {
                Some(ParseRecord::Object(_, e)) => e,
                _ => &[],
            };
            let keys = realm.own_enumerable_string_keys(id)?;
            for k in keys {
                let er = k.as_str().and_then(|ks| entries.iter().find(|(ek, _)| ek == ks)).map(|(_, r)| r);
                let nv = internalize(realm, id, k.clone(), reviver, er, src)?;
                if nv.is_undefined() {
                    realm.delete_property(id, &k)?;
                } else {
                    realm.define_own(id, k, Property::data(nv, ALL))?;
                }
            }
        }
    }
    realm.call(reviver.clone(), Value::Object(holder), &[key.to_value(), val, Value::Object(context)])
}

struct Stringifier {
    replacer_fn: Option<Value>,
    allow_list: Option<Vec<PropertyKey>>,
    gap: String,
    indent: String,
    stack: Vec<ObjectId>,
}

fn quote(s: &JsStr, out: &mut String) {
    out.push('"');
    let units = s.as_units();
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        match c {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            c if c < 0x20 => out.push_str(&format!("\\u{:04x}", c)),
            c if (0xD800..0xDC00).contains(&c) => {
                if i + 1 < units.len() && (0xDC00..0xE000).contains(&units[i + 1]) {
                    let cp = 0x10000 + ((c as u32 - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00);
                    out.push(char::from_u32(cp).unwrap());
                    i += 1;
                } else {
                    out.push_str(&format!("\\u{:04x}", c));
                }
            }
            c if (0xDC00..0xE000).contains(&c) => out.push_str(&format!("\\u{:04x}", c)),
            c => out.push(char::from_u32(c as u32).unwrap_or('\u{fffd}')),
        }
        i += 1;
    }
    out.push('"');
}

impl Stringifier {
    fn serialize_property(&mut self, realm: &mut Realm, holder: Value, key: PropertyKey, out: &mut String) -> JsResult<bool> {
        if crate::stack::exhausted() {
            return realm.range_error("Maximum call stack size exceeded");
        }
        let mut value = realm.get(holder.clone(), key.clone())?;
        if let V::Object(_) | V::BigInt(_) = value.v() {
            let to_json = realm.get_str(value.clone(), "toJSON")?;
            if realm.is_callable(&to_json) {
                value = realm.call(to_json, value, &[key.to_value()])?;
            }
        }
        if let Some(f) = &self.replacer_fn {
            value = realm.call(f.clone(), holder, &[key.to_value(), value])?;
        }
        // unwrap wrappers
        if let V::Object(id) = value.v() {
            // Raw JSON objects have a null prototype (cheap pre-check).
            if realm.obj(id).proto.is_none() && is_raw_json_object(realm, id) {
                let raw = realm.get_str(value.clone(), "rawJSON")?;
                let raw = realm.to_string(raw)?;
                out.push_str(&raw.to_rust_string());
                return Ok(true);
            }
            match &realm.obj(id).kind {
                ObjectKind::Number(_) => value = Value::Number(realm.to_number(value.clone())?),
                ObjectKind::String(_) => value = Value::String(realm.to_string(value.clone())?),
                ObjectKind::Boolean(b) => value = Value::Bool(*b),
                ObjectKind::BigInt(b) => value = Value::BigInt(b.clone()),
                _ => {}
            }
        }
        match value.v() {
            V::Null => out.push_str("null"),
            V::Bool(b) => out.push_str(if b { "true" } else { "false" }),
            V::String(s) => quote(&s, out),
            V::Number(n) => {
                if n.is_finite() {
                    out.push_str(&number_to_string(n));
                } else {
                    out.push_str("null");
                }
            }
            V::BigInt(_) => return realm.type_error("Do not know how to serialize a BigInt"),
            V::Object(id) if !realm.is_callable(&value) => {
                if array::is_array_value(realm, &value) {
                    self.serialize_array(realm, id, out)?;
                } else {
                    self.serialize_object(realm, id, out)?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn check_cycle(&mut self, realm: &mut Realm, id: ObjectId) -> JsResult<()> {
        if self.stack.contains(&id) {
            return realm.type_error("Converting circular structure to JSON");
        }
        Ok(())
    }

    fn serialize_object(&mut self, realm: &mut Realm, id: ObjectId, out: &mut String) -> JsResult<()> {
        self.check_cycle(realm, id)?;
        self.stack.push(id);
        let stepback = self.indent.clone();
        self.indent.push_str(&self.gap.clone());
        let keys = match &self.allow_list {
            Some(l) => l.clone(),
            None => realm.own_enumerable_string_keys(id)?,
        };
        // Members are written straight into `out`; a member whose value
        // does not serialize (undefined, functions, symbols) is cut again.
        out.push('{');
        let mut any = false;
        for k in keys {
            let mark = out.len();
            if any {
                out.push(',');
            }
            if !self.gap.is_empty() {
                out.push('\n');
                out.push_str(&self.indent);
            }
            quote(k.as_str().unwrap(), out);
            out.push(':');
            if !self.gap.is_empty() {
                out.push(' ');
            }
            if self.serialize_property(realm, Value::Object(id), k, out)? {
                any = true;
            } else {
                out.truncate(mark);
            }
        }
        if any && !self.gap.is_empty() {
            out.push('\n');
            out.push_str(&stepback);
        }
        out.push('}');
        self.stack.pop();
        self.indent = stepback;
        Ok(())
    }

    fn serialize_array(&mut self, realm: &mut Realm, id: ObjectId, out: &mut String) -> JsResult<()> {
        self.check_cycle(realm, id)?;
        self.stack.push(id);
        let stepback = self.indent.clone();
        self.indent.push_str(&self.gap.clone());
        let len = realm.length_of(Value::Object(id))?;
        out.push('[');
        for i in 0..len {
            if i > 0 {
                out.push(',');
            }
            if !self.gap.is_empty() {
                out.push('\n');
                out.push_str(&self.indent);
            }
            if !self.serialize_property(realm, Value::Object(id), PropertyKey::index(i as u32), out)? {
                out.push_str("null");
            }
        }
        if len > 0 && !self.gap.is_empty() {
            out.push('\n');
            out.push_str(&stepback);
        }
        out.push(']');
        self.stack.pop();
        self.indent = stepback;
        Ok(())
    }
}

fn stringify(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    let replacer = arg(args, 1);
    let space = arg(args, 2);
    let mut st = Stringifier { replacer_fn: None, allow_list: None, gap: String::new(), indent: String::new(), stack: Vec::new() };
    if realm.is_callable(&replacer) {
        st.replacer_fn = Some(replacer);
    } else if array::is_array_value(realm, &replacer) {
        let items = realm.create_list_from_array_like(replacer)?;
        let mut list: Vec<PropertyKey> = Vec::new();
        for it in items {
            let k = match it.v() {
                V::String(s) => Some(PropertyKey::String(s.clone())),
                V::Number(n) => Some(PropertyKey::String(JsStr::new(&number_to_string(n)))),
                V::Object(id) => match &realm.obj(id).kind {
                    ObjectKind::String(_) | ObjectKind::Number(_) => Some(PropertyKey::String(realm.to_string(it.clone())?)),
                    _ => None,
                },
                _ => None,
            };
            if let Some(k) = k {
                if !list.contains(&k) {
                    list.push(k);
                }
            }
        }
        st.allow_list = Some(list);
    }
    let space = match space.v() {
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::Number(_) => Value::Number(realm.to_number(space.clone())?),
            ObjectKind::String(_) => Value::String(realm.to_string(space.clone())?),
            _ => space.clone(),
        },
        _ => space.clone(),
    };
    st.gap = match space.v() {
        V::Number(n) => " ".repeat(to_integer(n).clamp(0.0, 10.0) as usize),
        V::String(s) => s.to_rust_string().chars().take(10).collect(),
        _ => String::new(),
    };
    let wrapper = realm.new_object();
    realm.define_property(wrapper, "", value, ALL);
    let mut out = String::new();
    if st.serialize_property(realm, Value::Object(wrapper), PropertyKey::str(""), &mut out)? {
        Ok(Value::str(&out))
    } else {
        Ok(Value::Undefined)
    }
}
