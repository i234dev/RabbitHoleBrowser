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
    Bool,
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
            "bool" => Self::Bool,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            "mat4" => Self::Mat4,
            _ => return None,
        })
    }
    pub fn n(self) -> usize {
        match self {
            Self::IntLiteral | Self::Bool | Self::Sampler2D | Self::Float => 1,
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
    Boolean(bool),
    Texture(Box<Expr>, Box<Expr>),
    Name(String),
    Global(String),
    Swizzle(Box<Expr>, Vec<usize>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Compare(Compare, Box<Expr>, Box<Expr>),
    Logical(Logical, Box<Expr>, Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Construct(Vec<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Call(Arc<Function>, Vec<Expr>),
}

#[derive(Clone, Copy, Debug)]
enum Compare { Less, LessEqual, Greater, GreaterEqual, Equal, NotEqual }
#[derive(Clone, Copy, Debug)]
enum Logical { And, Or, Xor }

#[derive(Clone, Debug)]
enum Statement {
    Assign(String, Expr),
    Block(Vec<Statement>),
    If(Expr, Vec<Statement>, Vec<Statement>),
    Return(Option<Expr>),
}
impl Statement {
    fn returns(&self) -> bool {
        match self {
            Self::Return(_) => true,
            Self::Block(body) => body.last().is_some_and(Self::returns),
            Self::If(_, yes, no) => yes.last().is_some_and(Self::returns) && no.last().is_some_and(Self::returns),
            Self::Assign(_, _) => false,
        }
    }
}
enum Flow { Next, Return(Option<Value>) }
fn execute(statements: &[Statement], env: &mut BTreeMap<String,Value>, globals: &BTreeMap<String,Value>, sample: &dyn Fn(i32,[f32;2])->[f32;4]) -> Flow {
    for statement in statements {
        let flow = match statement {
            Statement::Assign(name, expr) => {
                let value = expr.eval(env, globals, sample);
                env.insert(name.clone(), value);
                Flow::Next
            }
            Statement::Block(body) => execute(body,env,globals,sample),
            Statement::If(condition, yes, no) => execute(if condition.eval(env,globals,sample).v[0] != 0. { yes } else { no }, env,globals,sample),
            Statement::Return(expr) => Flow::Return(expr.as_ref().map(|expr| expr.eval(env,globals,sample))),
        };
        if !matches!(flow, Flow::Next) { return flow; }
    }
    Flow::Next
}

/// Every reachable return and fallthrough in main must have written its output.
/// Both branches remain checked even when a condition is a constant.
fn output_paths(statements: &[Statement], output: &str, mut written: bool) -> Result<Option<bool>,String> {
    for statement in statements {
        match statement {
            Statement::Assign(name, _) => written |= name == output,
            Statement::Block(body) => match output_paths(body,output,written)? { Some(value) => written=value, None => return Ok(None) },
            Statement::If(_,yes,no) => {
                written = match (output_paths(yes,output,written)?,output_paths(no,output,written)?) {
                    (Some(a),Some(b)) => a && b,
                    (Some(a),None) | (None,Some(a)) => a,
                    (None,None) => return Ok(None),
                };
            }
            Statement::Return(_) => {
                if !written { return Err(format!("main must write {output} before returning")); }
                return Ok(None);
            }
        }
    }
    Ok(Some(written))
}

/// Definitions form a DAG: calls can target only a previously defined helper.
/// Sharing bodies avoids exponential AST expansion while costs include every call.
#[derive(Clone, Debug)]
struct Function {
    result_type: Ty,
    parameters: Vec<(String, Ty)>,
    locals: BTreeMap<String, Symbol>,
    globals: BTreeSet<String>,
    statements: Vec<Statement>,
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
        match execute(&self.statements, &mut env, globals, sample) {
            Flow::Return(Some(value)) => value,
            _ => unreachable!("helpers have typed returns on every path"),
        }
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
    statements: Vec<Statement>,
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
        execute(&self.statements, &mut env, &globals, sample);
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
            ExprKind::Boolean(value) => Value::from(Ty::Bool, &[u8::from(*value) as f32]),
            ExprKind::Not(expr) => Value::from(Ty::Bool, &[u8::from(expr.eval(env,globals,sample).v[0] == 0.) as f32]),
            ExprKind::Logical(op,a,b) => {
                let a = a.eval(env,globals,sample).v[0] != 0.;
                let value = match op {
                    Logical::And => a && b.eval(env,globals,sample).v[0] != 0.,
                    Logical::Or => a || b.eval(env,globals,sample).v[0] != 0.,
                    Logical::Xor => a != (b.eval(env,globals,sample).v[0] != 0.),
                };
                Value::from(Ty::Bool,&[u8::from(value) as f32])
            }
            ExprKind::Compare(op,a,b) => {
                let a=a.eval(env,globals,sample);
                let b=b.eval(env,globals,sample);
                let value=match op {
                    Compare::Less => a.v[0] < b.v[0], Compare::LessEqual => a.v[0] <= b.v[0],
                    Compare::Greater => a.v[0] > b.v[0], Compare::GreaterEqual => a.v[0] >= b.v[0],
                    Compare::Equal => a.v[..a.ty.n()] == b.v[..b.ty.n()],
                    Compare::NotEqual => a.v[..a.ty.n()] != b.v[..b.ty.n()],
                };
                Value::from(Ty::Bool,&[u8::from(value) as f32])
            }
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
    Op(String),
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
        if let Some(op) = ["<=", ">=", "==", "!=", "&&", "||", "^^"].iter().find(|op| b.get(i..i+2) == Some(op.as_bytes())) {
            out.push(Tok::Op((*op).into())); i+=2; continue;
        }
        if b";{}(),=+-*/.!<>".contains(&c) {
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
    locals: BTreeMap<String, Symbol>,
    scopes: Vec<BTreeMap<String, Binding>>,
    function_globals: Option<BTreeSet<String>>,
    function_name: Option<String>,
    function_depth: usize,
    result_type: Option<Ty>,
    statement_depth: usize,
    statement_count: usize,
}
#[derive(Clone)]
struct Binding { key: String, symbol: Symbol }
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
        self.local(name).map(|binding| &binding.symbol)
            .or_else(|| self.shader.symbols.get(name))
    }
    fn local(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }
    fn read_globals(&mut self, names: impl IntoIterator<Item = String>) {
        if let Some(reads) = self.function_globals.as_mut() {
            reads.extend(names);
        } else {
            self.shader.reads.extend(names);
        }
    }
    fn read_name(&mut self, name: &str) -> Result<(), String> {
        if self.local(name).is_some() { return Ok(()); }
        if self.function_name.is_some() {
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
        let precision = matches!(n.as_str(), "lowp" | "mediump" | "highp");
        if precision {
            n = self.id()?;
        }
        let ty = Ty::parse(&n).ok_or_else(|| format!("unsupported type {n}"))?;
        if precision && ty == Ty::Bool { return Err("bool cannot have a precision qualifier".into()); }
        Ok(ty)
    }
    fn declare(&mut self, name: String, ty: Ty, qual: Qual) -> Result<String, String> {
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
                    | "switch"
                    | "case"
                    | "default"
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
            || if qual == Qual::Local { self.scopes.last().is_some_and(|scope| scope.contains_key(&name)) } else { self.shader.symbols.contains_key(&name) }
            || (qual != Qual::Local && self.functions.contains_key(&name))
        {
            return Err(format!("invalid or duplicate declaration {name}"));
        }
        if (if qual == Qual::Local { self.locals.len() + if self.function_name.is_none() { self.shader.symbols.len() } else {0} } else {self.shader.symbols.len()}) >= 64 {
            return Err("too many shader variables".into());
        }
        if qual == Qual::Local {
            let key = format!("@local{}",self.locals.len());
            let symbol = Symbol {ty,qual};
            self.scopes.last_mut().expect("locals require a scope").insert(name,Binding {key:key.clone(),symbol:symbol.clone()});
            self.locals.insert(key.clone(),symbol);
            Ok(key)
        } else {
            self.shader.symbols.insert(name.clone(), Symbol { ty, qual });
            Ok(name)
        }
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
        if declaration.is_none() && self.function_name.is_some() && self.local(&name).is_none()
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
        let target = if let Some(ty) = declaration {
            // GLSL ES 1.00 section 4.2 and 3.00 section 4.2.2: the new
            // name becomes visible only after its initializer. Until then,
            // an outer variable or helper of the same name remains visible.
            self.declare(name, ty, Qual::Local)?
        } else { self.local(&name).map_or(name.clone(),|binding|binding.key.clone()) };
        if self.function_name.is_none() && self.shader.symbols.contains_key(&target) { self.shader.writes.insert(target.clone()); }
        Ok((target, expr))
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
        self.locals = BTreeMap::new();
        self.scopes = vec![BTreeMap::new()];
        self.result_type = Some(result_type);
        self.statement_count = 0;
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
                if !matches!(ty, Ty::Bool | Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4) {
                    return Err("function parameters require bool, float or vector types".into());
                }
                let parameter = self.id()?;
                let key = self.declare(parameter, ty, Qual::Local)?;
                parameters.push((key, ty));
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
        let statements = self.statements()?;
        if !statements.last().is_some_and(Statement::returns) {
            return Err("user function must return a value on every path".into());
        }
        let locals = std::mem::take(&mut self.locals);
        self.scopes.clear();
        let globals = self.function_globals.take().unwrap();
        let cost = self.shader.cost + parameters.len() + locals.len() + globals.len() + 1;
        if cost > 512 {
            return Err("function exceeds expression budget".into());
        }
        let call_depth = self.function_depth;
        self.function_name = None;
        self.result_type = None;
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
                cost,
                call_depth,
            }),
        );
        Ok(())
    }
    fn statements(&mut self) -> Result<Vec<Statement>,String> {
        let mut statements = Vec::new();
        while !self.eat('}') {
            if statements.last().is_some_and(Statement::returns) {
                return Err("statements after an unconditional return are unsupported".into());
            }
            statements.push(self.statement()?);
        }
        Ok(statements)
    }
    fn branch(&mut self) -> Result<Vec<Statement>,String> {
        self.scopes.push(BTreeMap::new());
        let statement = self.statement()?;
        self.scopes.pop();
        Ok(vec![statement])
    }
    fn statement(&mut self) -> Result<Statement,String> {
        self.statement_depth += 1;
        self.statement_count += 1;
        self.shader.cost += 1;
        if self.statement_depth > 16 { return Err("statement nesting exceeds 16".into()); }
        if self.statement_count > 128 { return Err("too many shader statements".into()); }
        if self.shader.cost > 512 { return Err("shader exceeds expression budget".into()); }
        let statement = if self.eat('{') {
            self.scopes.push(BTreeMap::new());
            let body = self.statements()?;
            self.scopes.pop();
            Statement::Block(body)
        } else if self.eat_id("if") {
            self.expect(Tok::P('('))?;
            let condition = self.expr(0)?;
            if condition.ty != Ty::Bool { return Err("if requires a scalar bool condition".into()); }
            self.expect(Tok::P(')'))?;
            let yes = self.branch()?;
            let no = if self.eat_id("else") { self.branch()? } else { Vec::new() };
            Statement::If(condition,yes,no)
        } else if self.eat_id("return") {
            let value = if let Some(ty) = self.result_type {
                let expression = self.expr(0)?;
                if expression.ty != ty { return Err("return type mismatch".into()); }
                Some(expression)
            } else {None};
            self.expect(Tok::P(';'))?;
            Statement::Return(value)
        } else if self.eat(';') {
            Statement::Block(Vec::new())
        } else {
            let (name,expression) = self.assignment()?;
            Statement::Assign(name,expression)
        };
        self.statement_depth -= 1;
        Ok(statement)
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
                let e = self.expr(8)?;
                if matches!(e.ty,Ty::Sampler2D | Ty::Bool) {
                    return Err("unary minus requires a numeric value".into());
                }
                Expr {
                    ty: e.ty,
                    kind: ExprKind::Neg(Box::new(e)),
                }
            }
            Tok::P('+') => {
                let e = self.expr(8)?;
                if matches!(e.ty,Ty::Sampler2D | Ty::Bool) {
                    return Err("unary plus requires a numeric value".into());
                }
                e
            }
            Tok::P('!') => {
                let expr = self.expr(8)?;
                if expr.ty != Ty::Bool { return Err("logical not requires bool".into()); }
                Expr {ty:Ty::Bool,kind:ExprKind::Not(Box::new(expr))}
            }
            Tok::P('(') => {
                let e = self.expr(0)?;
                self.expect(Tok::P(')'))?;
                e
            }
            Tok::Id(name) => {
                if name == "true" || name == "false" {
                    Expr {ty:Ty::Bool,kind:ExprKind::Boolean(name=="true")}
                } else if (name == "texture2D" || name == "texture") && *self.peek() == Tok::P('(') {
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
                    if ty == Ty::Bool && (args.len()!=1 || args[0].ty != Ty::Bool)
                        || ty != Ty::Bool && args.iter().any(|arg| arg.ty == Ty::Bool) {
                        return Err("numeric/bool constructor conversions are unsupported".into());
                    }
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
                    let local = self.local(&name).map(|binding|binding.key.clone());
                    let global = self.function_name.is_some() && local.is_none();
                    self.read_name(&name)?;
                    Expr {
                        ty,
                        kind: if global {
                            ExprKind::Global(name)
                        } else {
                            ExprKind::Name(local.unwrap_or(name))
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
                    Ty::Mat4 | Ty::Float | Ty::IntLiteral | Ty::Sampler2D | Ty::Bool
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
            let (op,prec) = match self.peek() {
                Tok::P(c @ ('+' | '-')) => (c.to_string(),6),
                Tok::P(c @ ('*' | '/')) => (c.to_string(),7),
                Tok::P(c @ ('<' | '>')) => (c.to_string(),5),
                Tok::Op(op) => (op.clone(),match op.as_str() {
                    "||"=>1,"^^"=>2,"&&"=>3,"=="|"!="=>4,"<="|">="=>5,_=>unreachable!(),
                }),
                _ => break,
            };
            if prec < min_prec { break; }
            self.pos += 1;
            let right = self.expr(prec+1)?;
            let kind;
            let ty;
            if matches!(op.as_str(),"&&"|"||"|"^^") {
                if left.ty != Ty::Bool || right.ty != Ty::Bool {
                    return Err("logical operators require two bool operands".into());
                }
                let op=match op.as_str() {"&&"=>Logical::And,"||"=>Logical::Or,_=>Logical::Xor};
                ty=Ty::Bool;
                kind=ExprKind::Logical(op,Box::new(left),Box::new(right));
            } else if matches!(op.as_str(),"<"|"<="|">"|">="|"=="|"!=") {
                if matches!(op.as_str(),"=="|"!=") {
                    if left.ty != right.ty || matches!(left.ty,Ty::IntLiteral|Ty::Sampler2D) {
                        return Err("equality requires matching bool, float, vector or matrix operands".into());
                    }
                } else if left.ty != Ty::Float || right.ty != Ty::Float {
                    return Err("relational comparisons require two scalar float operands".into());
                }
                let op=match op.as_str() {"<"=>Compare::Less,"<="=>Compare::LessEqual,">"=>Compare::Greater,">="=>Compare::GreaterEqual,"=="=>Compare::Equal,_=>Compare::NotEqual};
                ty=Ty::Bool;
                kind=ExprKind::Compare(op,Box::new(left),Box::new(right));
            } else {
                if matches!(left.ty,Ty::Sampler2D|Ty::Bool) || matches!(right.ty,Ty::Sampler2D|Ty::Bool) {
                    return Err("arithmetic requires numeric operands".into());
                }
                if left.ty == Ty::IntLiteral || right.ty == Ty::IntLiteral {
                    return Err("integer arithmetic and implicit integer-to-float conversion are unsupported".into());
                }
                ty = if op == "*" && ((left.ty==Ty::Mat4 && right.ty==Ty::Vec4) || (left.ty==Ty::Vec4 && right.ty==Ty::Mat4)) {Ty::Vec4}
                    else if left.ty==right.ty {left.ty}
                    else if left.ty==Ty::Float {right.ty}
                    else if right.ty==Ty::Float {left.ty}
                    else {return Err("binary operand types do not match".into());};
                kind=ExprKind::Binary(op.as_bytes()[0] as char,Box::new(left),Box::new(right));
            }
            left=Expr {ty,kind};
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
        locals: BTreeMap::new(),
        scopes: Vec::new(),
        result_type: None,
        statement_depth: 0,
        statement_count: 0,
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
            "bool" | "float" | "vec2" | "vec3" | "vec4" | "lowp" | "mediump" | "highp"
        ) {
            if location.is_some() {
                return Err("layout cannot qualify a user function".into());
            }
            p.pos -= 1;
            let result_type = p.ty()?;
            if !matches!(result_type, Ty::Bool | Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4) {
                return Err("functions must return bool, float or vector types".into());
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
        if ty == Ty::Bool && qual != Qual::Uniform {
            return Err("bool interfaces other than uniforms are unsupported".into());
        }
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
    p.scopes.push(BTreeMap::new());
    p.statement_count=0;
    p.shader.statements=p.statements()?;
    p.expect(Tok::End)?;
    if output_paths(&p.shader.statements,&p.shader.output,false)? == Some(false) {
        return Err(format!("shader must write {} on every path",p.shader.output));
    }
    p.shader.symbols.extend(p.locals);
    Ok(p.shader)
}

#[cfg(test)]
mod control_flow_evaluation {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn logical_short_circuit_and_early_returns_skip_actual_sampler_execution() {
        for version in [100,300] {
            let sample=if version==300 {"texture"} else {"texture2D"};
            let output=if version==300 {"pixel"} else {"gl_FragColor"};
            let prefix=if version==300 {"#version 300 es\nprecision mediump float;out vec4 pixel;"} else {"precision mediump float;"};
            for (expression,expected_calls,expected_red) in [
                ("false&&touched()",0,false), ("true||touched()",0,true),
                ("true&&touched()",1,true), ("false||touched()",1,true),
                ("false^^touched()",1,true), ("true^^touched()",1,false),
                ("route(true)",0,true), ("route(false)",1,true),
            ] {
                let source=format!("{prefix}uniform sampler2D image;bool touched(){{return {sample}(image,vec2(0.5)).r>0.0;}}bool route(bool flag){{if(flag){{{{return true;}}}}return touched();}}void main(){{if({expression}){{{output}=vec4(1.0,0.0,0.0,1.0);}}else{{{output}=vec4(0.0,1.0,0.0,1.0);}}}}");
                let shader=compile(&source,false,version).unwrap();
                let calls=Cell::new(0);
                let result=shader.run_with_sampler(BTreeMap::new(),&|_,_| {calls.set(calls.get()+1);[1.,0.,0.,1.]});
                assert_eq!(calls.get(),expected_calls,"{version}: {expression}");
                assert_eq!(result[output].v[0],if expected_red {1.}else{0.},"{version}: {expression}");
            }
        }
    }
}
