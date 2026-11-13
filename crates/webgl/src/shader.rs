//! A deliberately bounded, versioned GLSL ES 1.00 / 3.00 subset. Unsupported grammar
//! produces a compile error; it is never replaced with a canned shader.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_FUNCTIONS: usize = 16;
const MAX_PARAMETERS: usize = 8;
const MAX_CALL_DEPTH: usize = 16;

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
    Global(String),
    Swizzle(Box<Expr>, Vec<usize>),
    Neg(Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Construct(Vec<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Call(Arc<Function>, Vec<Expr>),
}

/// Definitions form a DAG: calls can target only a previously defined helper.
/// Sharing bodies avoids exponential AST expansion while costs include every call.
#[derive(Clone, Debug)]
struct Function {
    result_type: Ty,
    parameters: Vec<(String, Ty)>,
    locals: BTreeMap<String, Symbol>,
    globals: BTreeSet<String>,
    statements: Vec<(String, Expr)>,
    result: Expr,
    cost: usize,
    call_depth: usize,
}
impl Function {
    fn eval(
        &self,
        globals: &BTreeMap<String, Value>,
        values: &[Value],
        sample: &dyn Fn(i32, [f32; 2]) -> [f32; 4],
    ) -> Value {
        let mut env = BTreeMap::new();
        for (name, symbol) in &self.locals {
            env.insert(name.clone(), Value::zero(symbol.ty));
        }
        for ((name, _), value) in self.parameters.iter().zip(values) {
            env.insert(name.clone(), *value);
        }
        for (name, expression) in &self.statements {
            let value = expression.eval(&env, globals, sample);
            env.insert(name.clone(), value);
        }
        self.result.eval(&env, globals, sample)
    }
}

/// Floating-point overloads shared by GLSL ES 1.00 and 3.00, sections 8.1–8.4.
/// Integer/bool overloads require types this bounded evaluator does not expose.
#[derive(Clone, Copy, Debug)]
enum Builtin {
    Radians,
    Degrees,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Pow,
    Exp,
    Log,
    Exp2,
    Log2,
    Sqrt,
    InverseSqrt,
    Abs,
    Sign,
    Floor,
    Ceil,
    Fract,
    Mod,
    Min,
    Max,
    Clamp,
    Mix,
    Step,
    SmoothStep,
    Length,
    Distance,
    Dot,
    Normalize,
    Cross,
}
impl Builtin {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "radians" => Self::Radians,
            "degrees" => Self::Degrees,
            "sin" => Self::Sin,
            "cos" => Self::Cos,
            "tan" => Self::Tan,
            "asin" => Self::Asin,
            "acos" => Self::Acos,
            "atan" => Self::Atan,
            "pow" => Self::Pow,
            "exp" => Self::Exp,
            "log" => Self::Log,
            "exp2" => Self::Exp2,
            "log2" => Self::Log2,
            "sqrt" => Self::Sqrt,
            "inversesqrt" => Self::InverseSqrt,
            "abs" => Self::Abs,
            "sign" => Self::Sign,
            "floor" => Self::Floor,
            "ceil" => Self::Ceil,
            "fract" => Self::Fract,
            "mod" => Self::Mod,
            "min" => Self::Min,
            "max" => Self::Max,
            "clamp" => Self::Clamp,
            "mix" => Self::Mix,
            "step" => Self::Step,
            "smoothstep" => Self::SmoothStep,
            "length" => Self::Length,
            "distance" => Self::Distance,
            "dot" => Self::Dot,
            "normalize" => Self::Normalize,
            "cross" => Self::Cross,
            _ => return None,
        })
    }
    fn result_type(self, args: &[Expr]) -> Result<Ty, String> {
        let types: Vec<Ty> = args.iter().map(|e| e.ty).collect();
        if types
            .iter()
            .any(|ty| !matches!(ty, Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4))
        {
            return Err(format!(
                "{self:?} requires floating-point scalar/vector arguments"
            ));
        }
        let result = match (self, types.as_slice()) {
            (Self::Cross, [Ty::Vec3, Ty::Vec3]) => Some(Ty::Vec3),
            (Self::Cross, _) => None,
            (Self::Length, [_]) => Some(Ty::Float),
            (Self::Length, _) => None,
            (Self::Dot | Self::Distance, [a, b]) if a == b => Some(Ty::Float),
            (Self::Dot | Self::Distance, _) => None,
            (Self::Min | Self::Max | Self::Mod, [a, b]) if a == b || *b == Ty::Float => Some(*a),
            (Self::Min | Self::Max | Self::Mod, _) => None,
            (Self::Pow, [a, b]) if a == b => Some(*a),
            (Self::Pow, _) => None,
            (Self::Clamp, [x, low, high])
                if (x == low && x == high) || (*low == Ty::Float && *high == Ty::Float) =>
            {
                Some(*x)
            }
            (Self::Clamp, _) => None,
            (Self::Mix, [x, y, a]) if x == y && (x == a || *a == Ty::Float) => Some(*x),
            (Self::Mix, _) => None,
            (Self::Step, [edge, x]) if edge == x || *edge == Ty::Float => Some(*x),
            (Self::Step, _) => None,
            (Self::SmoothStep, [low, high, x])
                if (low == x && high == x) || (*low == Ty::Float && *high == Ty::Float) =>
            {
                Some(*x)
            }
            (Self::SmoothStep, _) => None,
            (Self::Atan, [a, b]) if a == b => Some(*a),
            (Self::Atan, [_]) => Some(types[0]),
            (Self::Atan, _) => None,
            (_, [a]) => Some(*a),
            _ => None,
        };
        result.ok_or_else(|| format!("no supported {self:?} overload for {types:?}"))
    }
    fn eval(self, ty: Ty, args: &[Value]) -> Value {
        let a = args[0];
        let mut result = Value::zero(ty);
        match self {
            Self::Length | Self::Distance | Self::Dot => {
                result.v[0] = (0..a.ty.n())
                    .map(|i| match self {
                        Self::Dot => a.v[i] * args[1].v[i],
                        Self::Distance => (a.v[i] - args[1].v[i]).powi(2),
                        _ => a.v[i] * a.v[i],
                    })
                    .sum::<f32>();
                if !matches!(self, Self::Dot) {
                    result.v[0] = result.v[0].sqrt();
                }
            }
            Self::Normalize => {
                let length = a.v[..a.ty.n()].iter().map(|x| x * x).sum::<f32>().sqrt();
                for i in 0..a.ty.n() {
                    result.v[i] = a.v[i] / length;
                }
            }
            Self::Cross => {
                let b = args[1];
                result.v[..3].copy_from_slice(&[
                    a.v[1] * b.v[2] - a.v[2] * b.v[1],
                    a.v[2] * b.v[0] - a.v[0] * b.v[2],
                    a.v[0] * b.v[1] - a.v[1] * b.v[0],
                ]);
            }
            _ => {
                for i in 0..ty.n() {
                    let at =
                        |arg: usize| args[arg].v[if args[arg].ty == Ty::Float { 0 } else { i }];
                    let x = at(0);
                    result.v[i] = match self {
                        Self::Radians => x.to_radians(),
                        Self::Degrees => x.to_degrees(),
                        Self::Sin => x.sin(),
                        Self::Cos => x.cos(),
                        Self::Tan => x.tan(),
                        Self::Asin => x.asin(),
                        Self::Acos => x.acos(),
                        Self::Atan if args.len() == 2 => x.atan2(at(1)),
                        Self::Atan => x.atan(),
                        Self::Pow => x.powf(at(1)),
                        Self::Exp => x.exp(),
                        Self::Log => x.ln(),
                        Self::Exp2 => x.exp2(),
                        Self::Log2 => x.log2(),
                        Self::Sqrt => x.sqrt(),
                        Self::InverseSqrt => x.sqrt().recip(),
                        Self::Abs => x.abs(),
                        Self::Sign => {
                            if x > 0. {
                                1.
                            } else if x < 0. {
                                -1.
                            } else {
                                0.
                            }
                        }
                        Self::Floor => x.floor(),
                        Self::Ceil => x.ceil(),
                        Self::Fract => x - x.floor(),
                        Self::Mod => x - at(1) * (x / at(1)).floor(),
                        Self::Min => x.min(at(1)),
                        Self::Max => x.max(at(1)),
                        // GLSL leaves reversed bounds undefined; avoid host clamp's panic.
                        Self::Clamp => x.max(at(1)).min(at(2)),
                        Self::Mix => x * (1. - at(2)) + at(1) * at(2),
                        Self::Step => {
                            if at(1) < x {
                                0.
                            } else {
                                1.
                            }
                        }
                        Self::SmoothStep => {
                            let t = ((at(2) - x) / (at(1) - x)).max(0.).min(1.);
                            t * t * (3. - 2. * t)
                        }
                        _ => unreachable!("geometric function handled above"),
                    };
                }
            }
        }
        result
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Shader {
    pub version: u32,
    pub output: String,
    pub locations: BTreeMap<String, u32>,
    pub symbols: BTreeMap<String, Symbol>,
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    function_globals: BTreeSet<String>,
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
        // Ordinary shaders keep the zero-allocation empty map. Helpers capture
        // only immutable globals they actually read, not the caller's locals.
        let globals = self
            .function_globals
            .iter()
            .map(|name| (name.clone(), env[name]))
            .collect();
        for (name, expr) in &self.statements {
            let v = expr.eval(&env, &globals, sample);
            env.insert(name.clone(), v);
        }
        env
    }
}
impl Expr {
    fn eval(
        &self,
        env: &BTreeMap<String, Value>,
        globals: &BTreeMap<String, Value>,
        sample: &dyn Fn(i32, [f32; 2]) -> [f32; 4],
    ) -> Value {
        match &self.kind {
            ExprKind::Call(function, args) => {
                let mut values = [Value::zero(Ty::Float); MAX_PARAMETERS];
                for (i, arg) in args.iter().enumerate() {
                    values[i] = arg.eval(env, globals, sample);
                }
                function.eval(globals, &values[..args.len()], sample)
            }
            ExprKind::Builtin(function, args) => {
                let mut values = [Value::zero(Ty::Float); 3];
                for (i, arg) in args.iter().enumerate() {
                    values[i] = arg.eval(env, globals, sample);
                }
                function.eval(self.ty, &values[..args.len()])
            }
            ExprKind::Texture(unit, uv) => {
                let unit = unit.eval(env, globals, sample);
                let uv = uv.eval(env, globals, sample);
                Value::from(Ty::Vec4, &sample(unit.v[0] as i32, [uv.v[0], uv.v[1]]))
            }
            ExprKind::Number(n) => Value::from(self.ty, &[*n]),
            ExprKind::Name(n) => env[n],
            ExprKind::Global(n) => globals[n],
            ExprKind::Neg(e) => {
                let mut r = e.eval(env, globals, sample);
                for v in &mut r.v[..r.ty.n()] {
                    *v = -*v;
                }
                r
            }
            ExprKind::Swizzle(e, ix) => {
                let a = e.eval(env, globals, sample);
                let mut r = Value::zero(self.ty);
                for (i, &j) in ix.iter().enumerate() {
                    r.v[i] = a.v[j];
                }
                r
            }
            ExprKind::Construct(args) => {
                let values: Vec<_> = args.iter().map(|a| a.eval(env, globals, sample)).collect();
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
                let (a, b) = (a.eval(env, globals, sample), b.eval(env, globals, sample));
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
    functions: BTreeMap<String, Arc<Function>>,
    locals: Option<BTreeMap<String, Symbol>>,
    function_globals: Option<BTreeSet<String>>,
    function_name: Option<String>,
    function_depth: usize,
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
    fn eat_id(&mut self, name: &str) -> bool {
        if *self.peek() == Tok::Id(name.into()) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn variable(&self, name: &str) -> Option<&Symbol> {
        self.locals
            .as_ref()
            .and_then(|locals| locals.get(name))
            .or_else(|| self.shader.symbols.get(name))
    }
    fn read_globals(&mut self, names: impl IntoIterator<Item = String>) {
        if let Some(reads) = self.function_globals.as_mut() {
            reads.extend(names);
        } else {
            self.shader.reads.extend(names);
        }
    }
    fn read_name(&mut self, name: &str) -> Result<(), String> {
        if let Some(locals) = &self.locals {
            if locals.contains_key(name) {
                return Ok(());
            }
            let symbol = &self.shader.symbols[name];
            if !matches!(symbol.qual, Qual::Uniform | Qual::Attribute | Qual::Input)
                && !(symbol.qual == Qual::Varying && !self.vertex)
            {
                return Err(format!("helpers cannot read mutable global {name}"));
            }
        }
        self.read_globals([name.to_string()]);
        Ok(())
    }
    fn ty(&mut self) -> Result<Ty, String> {
        let mut n = self.id()?;
        if matches!(n.as_str(), "lowp" | "mediump" | "highp") {
            n = self.id()?;
        }
        Ty::parse(&n).ok_or_else(|| format!("unsupported type {n}"))
    }
    fn declare(&mut self, name: String, ty: Ty, qual: Qual) -> Result<(), String> {
        if self.shader.version == 300
            && qual != Qual::Local
            && (name == "texture" || Builtin::parse(&name).is_some())
        {
            return Err(format!(
                "global variable conflicts with built-in {name} function"
            ));
        }
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
            || self.locals.as_ref().map_or_else(
                || self.shader.symbols.contains_key(&name),
                |locals| locals.contains_key(&name),
            )
            || (qual != Qual::Local && self.functions.contains_key(&name))
        {
            return Err(format!("invalid or duplicate declaration {name}"));
        }
        if self
            .locals
            .as_ref()
            .map_or(self.shader.symbols.len(), BTreeMap::len)
            >= 64
        {
            return Err("too many shader variables".into());
        }
        if let Some(locals) = self.locals.as_mut() {
            locals.insert(name, Symbol { ty, qual });
        } else {
            self.shader.symbols.insert(name, Symbol { ty, qual });
        }
        Ok(())
    }
    fn assignment(&mut self) -> Result<(String, Expr), String> {
        let first = self.id()?;
        let (name, declaration) = if Ty::parse(&first).is_some()
            || matches!(first.as_str(), "lowp" | "mediump" | "highp")
        {
            self.pos -= 1;
            let ty = self.ty()?;
            let name = self.id()?;
            (name, Some(ty))
        } else {
            (first, None)
        };
        let sym = if let Some(ty) = declaration {
            Symbol {
                ty,
                qual: Qual::Local,
            }
        } else {
            self.variable(&name)
                .ok_or_else(|| format!("unknown assignment target {name}"))?
                .clone()
        };
        if declaration.is_none()
            && self
                .locals
                .as_ref()
                .is_some_and(|locals| !locals.contains_key(&name))
        {
            return Err(format!("helpers cannot write global {name}"));
        }
        if matches!(sym.qual, Qual::Attribute | Qual::Uniform | Qual::Input)
            || (sym.qual == Qual::Varying && !self.vertex)
        {
            return Err(format!("{name} is read-only"));
        }
        self.expect(Tok::P('='))?;
        let expr = self.expr(0)?;
        if expr.ty != sym.ty {
            return Err(format!("assignment type mismatch for {name}"));
        }
        self.expect(Tok::P(';'))?;
        if let Some(ty) = declaration {
            // GLSL ES 1.00 section 4.2 and 3.00 section 4.2.2: the new
            // name becomes visible only after its initializer. Until then,
            // an outer variable or helper of the same name remains visible.
            self.declare(name.clone(), ty, Qual::Local)?;
        }
        Ok((name, expr))
    }
    fn function(&mut self, name: String, result_type: Ty) -> Result<(), String> {
        if self.functions.len() >= MAX_FUNCTIONS {
            return Err("too many user functions".into());
        }
        if Builtin::parse(&name).is_some() || matches!(name.as_str(), "texture" | "texture2D") {
            return Err("redefining or overloading built-in functions is unsupported".into());
        }
        if self.functions.contains_key(&name) {
            return Err("user function overloading/redefinition is unsupported".into());
        }
        // Reuse global identifier validation without making a function a variable.
        self.declare(name.clone(), result_type, Qual::Uniform)?;
        self.shader.symbols.remove(&name);
        self.locals = Some(BTreeMap::new());
        self.function_globals = Some(BTreeSet::new());
        self.function_name = Some(name.clone());
        self.function_depth = 1;
        let outer_cost = std::mem::replace(&mut self.shader.cost, 0);
        self.expect(Tok::P('('))?;
        let mut parameters = Vec::new();
        if self.eat_id("void") {
            self.expect(Tok::P(')'))?;
        } else if !self.eat(')') {
            loop {
                if parameters.len() >= MAX_PARAMETERS {
                    return Err("too many function parameters".into());
                }
                if matches!(self.peek(), Tok::Id(q) if matches!(q.as_str(), "out" | "inout" | "const"))
                {
                    return Err("only ordinary or in function parameters are supported".into());
                }
                self.eat_id("in");
                let ty = self.ty()?;
                if !matches!(ty, Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4) {
                    return Err("function parameters require float or vector types".into());
                }
                let parameter = self.id()?;
                self.declare(parameter.clone(), ty, Qual::Local)?;
                parameters.push((parameter, ty));
                if self.eat(')') {
                    break;
                }
                self.expect(Tok::P(','))?;
            }
        }
        if self.eat(';') {
            return Err("function prototypes are unsupported; define helpers before use".into());
        }
        self.expect(Tok::P('{'))?;
        let mut statements = Vec::new();
        let result = loop {
            if self.eat_id("return") {
                let result = self.expr(0)?;
                if result.ty != result_type {
                    return Err(format!("return type mismatch in {name}"));
                }
                self.expect(Tok::P(';'))?;
                self.expect(Tok::P('}'))?;
                break result;
            }
            if self.eat('}') {
                return Err("user function requires one final return expression".into());
            }
            if statements.len() >= 128 {
                return Err("too many function statements".into());
            }
            statements.push(self.assignment()?);
        };
        let locals = self.locals.take().unwrap();
        let globals = self.function_globals.take().unwrap();
        let cost = self.shader.cost + parameters.len() + locals.len() + globals.len() + 1;
        if cost > 512 {
            return Err("function exceeds expression budget".into());
        }
        let call_depth = self.function_depth;
        self.function_name = None;
        self.function_depth = 0;
        self.shader.cost = outer_cost;
        self.functions.insert(
            name,
            Arc::new(Function {
                result_type,
                parameters,
                locals,
                globals,
                statements,
                result,
                cost,
                call_depth,
            }),
        );
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
                if (name == "texture2D" || name == "texture") && *self.peek() == Tok::P('(') {
                    if self.variable(&name).is_some() {
                        return Err(format!("{name} is a variable, not a callable function"));
                    }
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
                } else if let Some(function) =
                    Builtin::parse(&name).filter(|_| *self.peek() == Tok::P('('))
                {
                    if self.variable(&name).is_some() {
                        return Err(format!("{name} is a variable, not a callable function"));
                    }
                    self.expect(Tok::P('('))?;
                    let mut args = Vec::new();
                    if !self.eat(')') {
                        loop {
                            if args.len() >= 3 {
                                return Err("too many built-in function arguments".into());
                            }
                            args.push(self.expr(0)?);
                            if self.eat(')') {
                                break;
                            }
                            self.expect(Tok::P(','))?;
                        }
                    }
                    let ty = function.result_type(&args)?;
                    // Math calls remain within the shader's bounded execution budget.
                    self.shader.cost += 4 * ty.n();
                    if self.shader.cost > 512 {
                        return Err("shader exceeds expression budget".into());
                    }
                    Expr {
                        ty,
                        kind: ExprKind::Builtin(function, args),
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
                } else if *self.peek() == Tok::P('(') {
                    if self.variable(&name).is_some() {
                        return Err(format!("{name} is a variable, not a callable function"));
                    }
                    if self.function_name.as_deref() == Some(&name) {
                        return Err("recursive functions are unsupported".into());
                    }
                    let function = self.functions.get(&name).cloned().ok_or_else(|| format!("helper {name} must be defined before use; forward calls and recursion are unsupported"))?;
                    self.expect(Tok::P('('))?;
                    let mut args = Vec::new();
                    if !self.eat(')') {
                        loop {
                            if args.len() >= MAX_PARAMETERS {
                                return Err("too many function arguments".into());
                            }
                            args.push(self.expr(0)?);
                            if self.eat(')') {
                                break;
                            }
                            self.expect(Tok::P(','))?;
                        }
                    }
                    if args.len() != function.parameters.len()
                        || args
                            .iter()
                            .zip(&function.parameters)
                            .any(|(arg, (_, ty))| arg.ty != *ty)
                    {
                        return Err(format!(
                            "argument types or count do not match function {name}"
                        ));
                    }
                    let call_depth =
                        function.call_depth + usize::from(self.function_name.is_some());
                    if call_depth > MAX_CALL_DEPTH {
                        return Err("user function call nesting exceeds limit".into());
                    }
                    self.function_depth = self.function_depth.max(call_depth);
                    self.shader.cost += function.cost;
                    if self.shader.cost > 512 {
                        return Err("shader exceeds expression budget".into());
                    }
                    if self.function_name.is_none() {
                        self.shader
                            .function_globals
                            .extend(function.globals.iter().cloned());
                    }
                    self.read_globals(function.globals.iter().cloned());
                    Expr {
                        ty: function.result_type,
                        kind: ExprKind::Call(function, args),
                    }
                } else {
                    let sym = self
                        .variable(&name)
                        .ok_or_else(|| format!("unknown variable {name}"))?;
                    let ty = sym.ty;
                    let global = self
                        .locals
                        .as_ref()
                        .is_some_and(|locals| !locals.contains_key(&name));
                    self.read_name(&name)?;
                    Expr {
                        ty,
                        kind: if global {
                            ExprKind::Global(name)
                        } else {
                            ExprKind::Name(name)
                        },
                    }
                }
            }
            _ => return Err("expected expression; general control flow is not supported".into()),
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
            function_globals: BTreeSet::new(),
            statements: Vec::new(),
            cost: 0,
        },
        depth: 0,
        functions: BTreeMap::new(),
        locals: None,
        function_globals: None,
        function_name: None,
        function_depth: 0,
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
        if matches!(
            n.as_str(),
            "float" | "vec2" | "vec3" | "vec4" | "lowp" | "mediump" | "highp"
        ) {
            if location.is_some() {
                return Err("layout cannot qualify a user function".into());
            }
            p.pos -= 1;
            let result_type = p.ty()?;
            if !matches!(result_type, Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4) {
                return Err("functions must return float or vector types".into());
            }
            let name = p.id()?;
            p.function(name, result_type)?;
            continue;
        }
        if n == "void" {
            if location.is_some() {
                return Err("layout cannot qualify main".into());
            }
            p.expect(Tok::Id("main".into()))?;
            p.expect(Tok::P('('))?;
            p.eat_id("void");
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
        let (name, expr) = p.assignment()?;
        p.shader.writes.insert(name.clone());
        p.shader.statements.push((name, expr));
    }
    p.expect(Tok::End)?;
    if !p.shader.writes.contains(&p.shader.output) {
        return Err(format!("shader must write {}", p.shader.output));
    }
    Ok(p.shader)
}
