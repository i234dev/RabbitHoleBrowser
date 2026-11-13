//! A deliberately bounded, versioned GLSL ES 1.00 / 3.00 subset. Unsupported grammar
//! produces a compile error; it is never replaced with a canned shader.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    // Integer literals can be converted by constructors, not assigned or
    // used in arithmetic as if they were floating-point values.
    IntLiteral,
    Sampler2D,
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat4,
}
impl Ty {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sampler2D" => Self::Sampler2D,
            "float" => Self::Float,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            "mat4" => Self::Mat4,
            _ => return None,
        })
    }
    pub fn n(self) -> usize {
        match self {
            Self::IntLiteral | Self::Sampler2D | Self::Float => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
            Self::Vec4 => 4,
            Self::Mat4 => 16,
        }
    }
    fn vector(n: usize) -> Self {
        [Self::Float, Self::Vec2, Self::Vec3, Self::Vec4][n - 1]
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Value {
    pub ty: Ty,
    pub v: [f32; 16],
}
impl Value {
    pub fn zero(ty: Ty) -> Self {
        Self { ty, v: [0.; 16] }
    }
    pub fn from(ty: Ty, src: &[f32]) -> Self {
        let mut r = Self::zero(ty);
        r.v[..src.len()].copy_from_slice(src);
        r
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Qual {
    Attribute,
    Uniform,
    Varying,
    Local,
    Output,
    Input,
}
#[derive(Clone, Debug)]
pub(crate) struct Symbol {
    pub ty: Ty,
    pub qual: Qual,
}
#[derive(Clone, Debug)]
struct Expr {
    ty: Ty,
    kind: ExprKind,
}
#[derive(Clone, Debug)]
enum ExprKind {
    Number(f32),
    Texture(Box<Expr>, Box<Expr>),
    Name(String),
    Swizzle(Box<Expr>, Vec<usize>),
    Neg(Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Construct(Vec<Expr>),
}
#[derive(Clone, Debug)]
pub(crate) struct Shader {
    pub version: u32,
    pub output: String,
    pub locations: BTreeMap<String, u32>,
    pub symbols: BTreeMap<String, Symbol>,
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    statements: Vec<(String, Expr)>,
    pub cost: usize,
}
impl Shader {
    pub fn run(&self, env: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        self.run_with_sampler(env, &|_, _| {
            unreachable!("vertex shaders cannot sample textures")
        })
    }
    pub fn run_with_sampler(
        &self,
        mut env: BTreeMap<String, Value>,
        sample: &dyn Fn(i32, [f32; 2]) -> [f32; 4],
    ) -> BTreeMap<String, Value> {
        for (name, sym) in &self.symbols {
            env.entry(name.clone())
                .or_insert_with(|| Value::zero(sym.ty));
        }
        for (name, expr) in &self.statements {
            let v = expr.eval(&env, sample);
            env.insert(name.clone(), v);
        }
        env
    }
}
impl Expr {
    fn eval(
        &self,
        env: &BTreeMap<String, Value>,
        sample: &dyn Fn(i32, [f32; 2]) -> [f32; 4],
    ) -> Value {
        match &self.kind {
            ExprKind::Texture(unit, uv) => {
                let unit = unit.eval(env, sample);
                let uv = uv.eval(env, sample);
                Value::from(Ty::Vec4, &sample(unit.v[0] as i32, [uv.v[0], uv.v[1]]))
            }
            ExprKind::Number(n) => Value::from(self.ty, &[*n]),
            ExprKind::Name(n) => env[n],
            ExprKind::Neg(e) => {
                let mut r = e.eval(env, sample);
                for v in &mut r.v[..r.ty.n()] {
                    *v = -*v;
                }
                r
            }
            ExprKind::Swizzle(e, ix) => {
                let a = e.eval(env, sample);
                let mut r = Value::zero(self.ty);
                for (i, &j) in ix.iter().enumerate() {
                    r.v[i] = a.v[j];
                }
                r
            }
            ExprKind::Construct(args) => {
                let values: Vec<_> = args.iter().map(|a| a.eval(env, sample)).collect();
                let mut r = Value::zero(self.ty);
                if values.len() == 1 && matches!(values[0].ty, Ty::Float | Ty::IntLiteral) {
                    if self.ty == Ty::Mat4 {
                        for i in 0..4 {
                            r.v[i * 5] = values[0].v[0];
                        }
                    } else {
                        r.v[..self.ty.n()].fill(values[0].v[0]);
                    }
                } else {
                    let mut i = 0;
                    for v in values {
                        for j in 0..v.ty.n() {
                            if i < self.ty.n() {
                                r.v[i] = v.v[j];
                                i += 1;
                            }
                        }
                    }
                }
                r
            }
            ExprKind::Binary(op, a, b) => {
                let (a, b) = (a.eval(env, sample), b.eval(env, sample));
                let mut r = Value::zero(self.ty);
                if *op == '*' && a.ty == Ty::Mat4 && b.ty == Ty::Vec4 {
                    for row in 0..4 {
                        r.v[row] = (0..4).map(|col| a.v[col * 4 + row] * b.v[col]).sum();
                    }
                } else if *op == '*' && a.ty == Ty::Vec4 && b.ty == Ty::Mat4 {
                    for col in 0..4 {
                        r.v[col] = (0..4).map(|row| a.v[row] * b.v[col * 4 + row]).sum();
                    }
                } else if *op == '*' && a.ty == Ty::Mat4 && b.ty == Ty::Mat4 {
                    for col in 0..4 {
                        for row in 0..4 {
                            r.v[col * 4 + row] =
                                (0..4).map(|k| a.v[k * 4 + row] * b.v[col * 4 + k]).sum();
                        }
                    }
                } else {
                    for i in 0..self.ty.n() {
                        let x = a.v[if a.ty == Ty::Float { 0 } else { i }];
                        let y = b.v[if b.ty == Ty::Float { 0 } else { i }];
                        r.v[i] = match op {
                            '+' => x + y,
                            '-' => x - y,
                            '*' => x * y,
                            '/' => x / y,
                            _ => unreachable!(),
                        };
                    }
                }
                r
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Id(String),
    Num(f32, bool),
    P(char),
    End,
}
fn lex(src: &str, token_limit: usize) -> Result<(Vec<Tok>, u32), String> {
    if src.len() > 65536 {
        return Err("shader source exceeds 64 KiB".into());
    }
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut version = 100;
    let mut version_seen = false;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b.get(i..i + 2) == Some(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b.get(i..i + 2) == Some(b"/*") {
            i += 2;
            while i + 1 < b.len() && &b[i..i + 2] != b"*/" {
                i += 1;
            }
            if i + 1 == b.len() || i == b.len() {
                return Err("unterminated comment".into());
            }
            i += 2;
            continue;
        }
        if c == b'#' {
            let s = i;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            if version_seen || !out.is_empty() {
                return Err("#version must appear once before source tokens".into());
            }
            let words = src[s..i]
                .split("//")
                .next()
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>();
            version = match words.as_slice() {
                ["#version", "100"] => 100,
                ["#version", "300", "es"] => 300,
                _ => return Err("supported versions are #version 100 and #version 300 es".into()),
            };
            version_seen = true;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if i - s > token_limit {
                return Err(format!("identifier exceeds {token_limit} characters"));
            }
            out.push(Tok::Id(src[s..i].into()));
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let s = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let n: f32 = src[s..i].parse().map_err(|_| "invalid numeric literal")?;
            if !n.is_finite() {
                return Err("numeric literal is not finite".into());
            }
            let integral = !src[s..i].contains(['.', 'e', 'E']);
            if integral && (n > 16_777_216. || (i - s > 1 && b[s] == b'0')) {
                return Err("integer literal is outside the supported decimal range".into());
            }
            out.push(Tok::Num(n, integral));
            continue;
        }
        if b";{}(),=+-*/.".contains(&c) {
            out.push(Tok::P(c as char));
            i += 1;
            continue;
        }
        return Err(format!("unsupported token at byte {i}"));
    }
    out.push(Tok::End);
    Ok((out, version))
}
struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
    vertex: bool,
    shader: Shader,
    depth: usize,
}
impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos]
    }
    fn next(&mut self) -> Tok {
        let t = self.tokens[self.pos].clone();
        if t != Tok::End {
            self.pos += 1;
        }
        t
    }
    fn id(&mut self) -> Result<String, String> {
        match self.next() {
            Tok::Id(s) => Ok(s),
            _ => Err("expected identifier".into()),
        }
    }
    fn expect(&mut self, t: Tok) -> Result<(), String> {
        if self.next() == t {
            Ok(())
        } else {
            Err(format!("expected {t:?}"))
        }
    }
    fn eat(&mut self, c: char) -> bool {
        if *self.peek() == Tok::P(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn ty(&mut self) -> Result<Ty, String> {
        let mut n = self.id()?;
        if matches!(n.as_str(), "lowp" | "mediump" | "highp") {
            n = self.id()?;
        }
        Ty::parse(&n).ok_or_else(|| format!("unsupported type {n}"))
    }
    fn declare(&mut self, name: String, ty: Ty, qual: Qual) -> Result<(), String> {
        if ty == Ty::Sampler2D && (qual != Qual::Uniform || self.vertex) {
            return Err("sampler2D is supported only as a fragment uniform".into());
        }
        if name.starts_with("gl_")
            || name.starts_with("webgl_")
            || name.starts_with("_webgl_")
            || Ty::parse(&name).is_some()
            || matches!(
                name.as_str(),
                "void"
                    | "layout"
                    | "main"
                    | "attribute"
                    | "uniform"
                    | "varying"
                    | "precision"
                    | "const"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "do"
                    | "return"
                    | "discard"
                    | "break"
                    | "continue"
                    | "struct"
                    | "bool"
                    | "int"
                    | "true"
                    | "false"
                    | "in"
                    | "out"
                    | "inout"
                    | "lowp"
                    | "mediump"
                    | "highp"
            )
            || self.shader.symbols.contains_key(&name)
        {
            return Err(format!("invalid or duplicate declaration {name}"));
        }
        if self.shader.symbols.len() >= 64 {
            return Err("too many shader variables".into());
        }
        self.shader.symbols.insert(name, Symbol { ty, qual });
        Ok(())
    }
    fn expr(&mut self, min_prec: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("expression nesting exceeds 64".into());
        }
        self.shader.cost += 1;
        if self.shader.cost > 512 {
            return Err("shader exceeds expression budget".into());
        }
        let mut left = match self.next() {
            Tok::Num(n, integral) => Expr {
                ty: if integral { Ty::IntLiteral } else { Ty::Float },
                kind: ExprKind::Number(n),
            },
            Tok::P('-') => {
                let e = self.expr(3)?;
                if e.ty == Ty::Sampler2D {
                    return Err("samplers cannot be negated".into());
                }
                Expr {
                    ty: e.ty,
                    kind: ExprKind::Neg(Box::new(e)),
                }
            }
            Tok::P('+') => {
                let e = self.expr(3)?;
                if e.ty == Ty::Sampler2D {
                    return Err("samplers are opaque".into());
                }
                e
            }
            Tok::P('(') => {
                let e = self.expr(0)?;
                self.expect(Tok::P(')'))?;
                e
            }
            Tok::Id(name) => {
                if name == "texture2D" || name == "texture" {
                    let expected = if self.shader.version == 100 {
                        "texture2D"
                    } else {
                        "texture"
                    };
                    if name != expected {
                        return Err(format!(
                            "{name} is unavailable in GLSL ES {}",
                            self.shader.version
                        ));
                    }
                    if self.vertex {
                        return Err("vertex texture sampling is unsupported".into());
                    }
                    self.expect(Tok::P('('))?;
                    let unit = self.expr(0)?;
                    self.expect(Tok::P(','))?;
                    let uv = self.expr(0)?;
                    self.expect(Tok::P(')'))?;
                    if unit.ty != Ty::Sampler2D || uv.ty != Ty::Vec2 {
                        return Err("texture2D requires sampler2D and vec2 arguments".into());
                    }
                    Expr {
                        ty: Ty::Vec4,
                        kind: ExprKind::Texture(Box::new(unit), Box::new(uv)),
                    }
                } else if let Some(ty) = Ty::parse(&name) {
                    if ty == Ty::Sampler2D {
                        return Err("samplers cannot be constructed".into());
                    }
                    self.expect(Tok::P('('))?;
                    let mut args = Vec::new();
                    loop {
                        args.push(self.expr(0)?);
                        if !self.eat(',') {
                            break;
                        }
                        if args.len() > 16 {
                            return Err("too many constructor arguments".into());
                        }
                    }
                    self.expect(Tok::P(')'))?;
                    let total: usize = args.iter().map(|e| e.ty.n()).sum();
                    let scalar =
                        args.len() == 1 && matches!(args[0].ty, Ty::Float | Ty::IntLiteral);
                    let trunc = args.len() == 1
                        && args[0].ty != Ty::Mat4
                        && total >= ty.n()
                        && ty != Ty::Mat4;
                    if !scalar && !trunc && total != ty.n() {
                        return Err("constructor component count mismatch".into());
                    }
                    if ty == Ty::Float && !scalar {
                        return Err("float constructors require a scalar argument".into());
                    }
                    if args.iter().any(|a| a.ty == Ty::Sampler2D) {
                        return Err("samplers cannot be converted".into());
                    }
                    if args.iter().any(|a| a.ty == Ty::Mat4) {
                        return Err("matrix conversion constructors are unsupported".into());
                    }
                    Expr {
                        ty,
                        kind: ExprKind::Construct(args),
                    }
                } else {
                    let sym = self
                        .shader
                        .symbols
                        .get(&name)
                        .ok_or_else(|| format!("unknown variable {name}"))?;
                    let ty = sym.ty;
                    self.shader.reads.insert(name.clone());
                    Expr {
                        ty,
                        kind: ExprKind::Name(name),
                    }
                }
            }
            _ => {
                return Err(
                    "expected expression; control flow and functions are not supported".into(),
                )
            }
        };
        loop {
            if self.eat('.') {
                if matches!(
                    left.ty,
                    Ty::Mat4 | Ty::Float | Ty::IntLiteral | Ty::Sampler2D
                ) {
                    return Err("swizzles require a vector".into());
                }
                let s = self.id()?;
                if s.is_empty() || s.len() > 4 {
                    return Err("invalid swizzle".into());
                }
                let mut group = None;
                let mut ix = Vec::new();
                for c in s.chars() {
                    let (g, i) = if let Some(i) = "xyzw".find(c) {
                        (0, i)
                    } else if let Some(i) = "rgba".find(c) {
                        (1, i)
                    } else if let Some(i) = "stpq".find(c) {
                        (2, i)
                    } else {
                        return Err("invalid swizzle".into());
                    };
                    if group.is_some_and(|x| x != g) || i >= left.ty.n() {
                        return Err("invalid swizzle components".into());
                    }
                    group = Some(g);
                    ix.push(i);
                }
                left = Expr {
                    ty: Ty::vector(ix.len()),
                    kind: ExprKind::Swizzle(Box::new(left), ix),
                };
                continue;
            }
            let op = match self.peek() {
                Tok::P(c @ ('+' | '-' | '*' | '/')) => *c,
                _ => break,
            };
            let prec = if op == '+' || op == '-' { 1 } else { 2 };
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            let right = self.expr(prec + 1)?;
            if left.ty == Ty::Sampler2D || right.ty == Ty::Sampler2D {
                return Err("samplers are opaque".into());
            }
            if left.ty == Ty::IntLiteral || right.ty == Ty::IntLiteral {
                return Err(
                    "integer arithmetic and implicit integer-to-float conversion are unsupported"
                        .into(),
                );
            }
            let ty = if op == '*'
                && ((left.ty == Ty::Mat4 && right.ty == Ty::Vec4)
                    || (left.ty == Ty::Vec4 && right.ty == Ty::Mat4))
            {
                Ty::Vec4
            } else if left.ty == right.ty {
                left.ty
            } else if left.ty == Ty::Float {
                right.ty
            } else if right.ty == Ty::Float {
                left.ty
            } else {
                return Err("binary operand types do not match".into());
            };
            left = Expr {
                ty,
                kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
            };
        }
        self.depth -= 1;
        Ok(left)
    }
}
pub(crate) fn compile(source: &str, vertex: bool, max_version: u32) -> Result<Shader, String> {
    let token_limit = if max_version >= 300 { 1024 } else { 256 };
    let (tokens, version) = lex(source, token_limit)?;
    if version > max_version {
        return Err(format!(
            "this context permits GLSL ES {max_version}, not {version}"
        ));
    }
    let mut p = Parser {
        tokens,
        pos: 0,
        vertex,
        shader: Shader {
            version,
            output: if vertex {
                "gl_Position".into()
            } else if version == 100 {
                "gl_FragColor".into()
            } else {
                String::new()
            },
            locations: BTreeMap::new(),
            symbols: BTreeMap::new(),
            reads: BTreeSet::new(),
            writes: BTreeSet::new(),
            statements: Vec::new(),
            cost: 0,
        },
        depth: 0,
    };
    if vertex || version == 100 {
        p.shader.symbols.insert(
            p.shader.output.clone(),
            Symbol {
                ty: Ty::Vec4,
                qual: Qual::Output,
            },
        );
    }
    if !vertex {
        p.shader.symbols.insert(
            "gl_FragCoord".into(),
            Symbol {
                ty: Ty::Vec4,
                qual: Qual::Input,
            },
        );
    }
    let mut precision_float = false;
    loop {
        let mut n = p.id()?;
        let mut location = None;
        if n == "layout" {
            if version != 300 {
                return Err("layout qualifiers require GLSL ES 3.00".into());
            }
            p.expect(Tok::P('('))?;
            p.expect(Tok::Id("location".into()))?;
            p.expect(Tok::P('='))?;
            let value = match p.next() {
                Tok::Num(n, true) if n >= 0. && n < crate::MAX_VERTEX_ATTRIBS as f32 => n as u32,
                _ => return Err("location must be a supported decimal integer literal".into()),
            };
            location = Some(value);
            p.expect(Tok::P(')'))?;
            n = p.id()?;
        }
        if n == "precision" {
            if location.is_some() {
                return Err("layout cannot qualify a precision declaration".into());
            }
            let q = p.id()?;
            if !matches!(q.as_str(), "lowp" | "mediump" | "highp") {
                return Err("invalid precision".into());
            }
            let ty = p.id()?;
            match ty.as_str() {
                "float" => precision_float = true,
                "int" | "sampler2D" => {}
                _ => return Err("unsupported precision declaration type".into()),
            }
            p.expect(Tok::P(';'))?;
            continue;
        }
        if n == "void" {
            if location.is_some() {
                return Err("layout cannot qualify main".into());
            }
            p.expect(Tok::Id("main".into()))?;
            p.expect(Tok::P('('))?;
            p.expect(Tok::P(')'))?;
            p.expect(Tok::P('{'))?;
            break;
        }
        let qual = match (n.as_str(), version, vertex) {
            ("uniform", _, _) => Qual::Uniform,
            ("attribute", 100, true) => Qual::Attribute,
            ("varying", 100, _) => Qual::Varying,
            ("in", 300, true) => Qual::Attribute,
            ("in", 300, false) => Qual::Varying,
            ("out", 300, true) => Qual::Varying,
            ("out", 300, false) => Qual::Output,
            _ => return Err(format!("unsupported declaration {n} for GLSL ES {version}")),
        };
        if location.is_some()
            && !((vertex && qual == Qual::Attribute) || (!vertex && qual == Qual::Output))
        {
            return Err(
                "locations are supported only on vertex inputs and fragment outputs".into(),
            );
        }
        if !vertex && location.is_some_and(|v| v != 0) {
            return Err(
                "multiple render targets are unsupported; output location must be zero".into(),
            );
        }
        let ty = p.ty()?;
        if ty == Ty::Mat4 && qual != Qual::Uniform {
            return Err("mat4 is supported only for uniforms and locals".into());
        }
        if qual == Qual::Output && ty != Ty::Vec4 {
            return Err("only a vec4 fragment output is supported".into());
        }
        let name = p.id()?;
        p.declare(name.clone(), ty, qual)?;
        if qual == Qual::Output {
            if !p.shader.output.is_empty() {
                return Err("multiple fragment outputs are unsupported".into());
            }
            p.shader.output = name.clone();
        }
        if let Some(location) = location {
            p.shader.locations.insert(name, location);
        }
        p.expect(Tok::P(';'))?;
    }
    if !vertex && !precision_float {
        return Err("fragment shaders require a float precision declaration".into());
    }
    if p.shader.output.is_empty() {
        return Err("GLSL ES 3.00 fragment shaders require one vec4 output".into());
    }
    while !p.eat('}') {
        if p.shader.statements.len() >= 128 {
            return Err("too many shader statements".into());
        }
        let first = p.id()?;
        let name = if Ty::parse(&first).is_some()
            || matches!(first.as_str(), "lowp" | "mediump" | "highp")
        {
            p.pos -= 1;
            let ty = p.ty()?;
            let name = p.id()?;
            p.declare(name.clone(), ty, Qual::Local)?;
            name
        } else {
            first
        };
        let sym = p
            .shader
            .symbols
            .get(&name)
            .ok_or_else(|| format!("unknown assignment target {name}"))?
            .clone();
        if matches!(sym.qual, Qual::Attribute | Qual::Uniform | Qual::Input)
            || (sym.qual == Qual::Varying && !vertex)
        {
            return Err(format!("{name} is read-only"));
        }
        p.expect(Tok::P('='))?;
        let expr = p.expr(0)?;
        if expr.ty != sym.ty {
            return Err(format!("assignment type mismatch for {name}"));
        }
        p.expect(Tok::P(';'))?;
        p.shader.writes.insert(name.clone());
        p.shader.statements.push((name, expr));
    }
    p.expect(Tok::End)?;
    if !p.shader.writes.contains(&p.shader.output) {
        return Err(format!("shader must write {}", p.shader.output));
    }
    Ok(p.shader)
}
