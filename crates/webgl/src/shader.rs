//! A deliberately bounded, typed GLSL ES 1.00 subset. Unsupported grammar
//! produces a compile error; it is never replaced with a canned shader.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat4,
}
impl Ty {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
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
            Self::Float => 1,
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
    Name(String),
    Swizzle(Box<Expr>, Vec<usize>),
    Neg(Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Construct(Vec<Expr>),
}
#[derive(Clone, Debug)]
pub(crate) struct Shader {
    pub symbols: BTreeMap<String, Symbol>,
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    statements: Vec<(String, Expr)>,
    pub cost: usize,
}
impl Shader {
    pub fn run(&self, mut env: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        for (name, sym) in &self.symbols {
            env.entry(name.clone())
                .or_insert_with(|| Value::zero(sym.ty));
        }
        for (name, expr) in &self.statements {
            let v = expr.eval(&env);
            env.insert(name.clone(), v);
        }
        env
    }
}
impl Expr {
    fn eval(&self, env: &BTreeMap<String, Value>) -> Value {
        match &self.kind {
            ExprKind::Number(n) => Value::from(Ty::Float, &[*n]),
            ExprKind::Name(n) => env[n],
            ExprKind::Neg(e) => {
                let mut r = e.eval(env);
                for v in &mut r.v[..r.ty.n()] {
                    *v = -*v;
                }
                r
            }
            ExprKind::Swizzle(e, ix) => {
                let a = e.eval(env);
                let mut r = Value::zero(self.ty);
                for (i, &j) in ix.iter().enumerate() {
                    r.v[i] = a.v[j];
                }
                r
            }
            ExprKind::Construct(args) => {
                let values: Vec<_> = args.iter().map(|a| a.eval(env)).collect();
                let mut r = Value::zero(self.ty);
                if values.len() == 1 && values[0].ty == Ty::Float {
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
                let (a, b) = (a.eval(env), b.eval(env));
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
    Num(f32),
    P(char),
    End,
}
fn lex(src: &str) -> Result<Vec<Tok>, String> {
    if src.len() > 65536 {
        return Err("shader source exceeds 64 KiB".into());
    }
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
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
            if src[s..i].split_whitespace().collect::<Vec<_>>() != ["#version", "100"]
                || !out.is_empty()
            {
                return Err("only #version 100 is supported".into());
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
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
            out.push(Tok::Num(n));
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
    Ok(out)
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
        if name.starts_with("gl_")
            || Ty::parse(&name).is_some()
            || matches!(
                name.as_str(),
                "void"
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
            Tok::Num(n) => Expr {
                ty: Ty::Float,
                kind: ExprKind::Number(n),
            },
            Tok::P('-') => {
                let e = self.expr(3)?;
                Expr {
                    ty: e.ty,
                    kind: ExprKind::Neg(Box::new(e)),
                }
            }
            Tok::P('+') => self.expr(3)?,
            Tok::P('(') => {
                let e = self.expr(0)?;
                self.expect(Tok::P(')'))?;
                e
            }
            Tok::Id(name) => {
                if let Some(ty) = Ty::parse(&name) {
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
                    let scalar = args.len() == 1 && args[0].ty == Ty::Float;
                    let trunc = args.len() == 1
                        && args[0].ty != Ty::Mat4
                        && total >= ty.n()
                        && ty != Ty::Mat4;
                    if !scalar && !trunc && total != ty.n() {
                        return Err("constructor component count mismatch".into());
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
                if matches!(left.ty, Ty::Mat4 | Ty::Float) {
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
pub(crate) fn compile(source: &str, vertex: bool) -> Result<Shader, String> {
    let mut p = Parser {
        tokens: lex(source)?,
        pos: 0,
        vertex,
        shader: Shader {
            symbols: BTreeMap::new(),
            reads: BTreeSet::new(),
            writes: BTreeSet::new(),
            statements: Vec::new(),
            cost: 0,
        },
        depth: 0,
    };
    let output = if vertex {
        "gl_Position"
    } else {
        "gl_FragColor"
    };
    p.shader.symbols.insert(
        output.into(),
        Symbol {
            ty: Ty::Vec4,
            qual: Qual::Output,
        },
    );
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
        let n = p.id()?;
        if n == "precision" {
            let q = p.id()?;
            if !matches!(q.as_str(), "lowp" | "mediump" | "highp") {
                return Err("invalid precision".into());
            }
            if p.ty()? != Ty::Float {
                return Err("only float precision declarations are supported".into());
            }
            precision_float = true;
            p.expect(Tok::P(';'))?;
            continue;
        }
        if n == "void" {
            p.expect(Tok::Id("main".into()))?;
            p.expect(Tok::P('('))?;
            p.expect(Tok::P(')'))?;
            p.expect(Tok::P('{'))?;
            break;
        }
        let qual = match n.as_str() {
            "attribute" if p.vertex => Qual::Attribute,
            "uniform" => Qual::Uniform,
            "varying" => Qual::Varying,
            _ => return Err(format!("unsupported global declaration {n}")),
        };
        let ty = p.ty()?;
        if ty == Ty::Mat4 && qual != Qual::Uniform {
            return Err("mat4 is supported only for uniforms and locals".into());
        }
        let name = p.id()?;
        p.declare(name, ty, qual)?;
        p.expect(Tok::P(';'))?;
    }
    if !vertex && !precision_float {
        return Err("fragment shaders require a float precision declaration".into());
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
    if !p.shader.writes.contains(output) {
        return Err(format!("shader must write {output}"));
    }
    Ok(p.shader)
}
