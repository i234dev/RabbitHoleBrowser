//! A deliberately bounded, versioned GLSL ES 1.00 / 3.00 subset. Unsupported grammar
//! produces a compile error; it is never replaced with a canned shader.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_FUNCTIONS: usize = 16;
const MAX_PARAMETERS: usize = 8;
const MAX_CALL_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    Int,
    IVec2,
    IVec3,
    IVec4,
    Bool,
    Sampler2D,
    Float,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
}
impl Ty {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sampler2D" => Self::Sampler2D,
            "float" => Self::Float,
            "bool" => Self::Bool,
            "int" => Self::Int,
            "ivec2" => Self::IVec2,
            "ivec3" => Self::IVec3,
            "ivec4" => Self::IVec4,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            "mat2" => Self::Mat2,
            "mat3" => Self::Mat3,
            "mat4" => Self::Mat4,
            _ => return None,
        })
    }
    pub fn n(self) -> usize {
        match self {
            Self::Int | Self::Bool | Self::Sampler2D | Self::Float => 1,
            Self::Vec2 | Self::IVec2 => 2,
            Self::Vec3 | Self::IVec3 => 3,
            Self::Vec4 | Self::IVec4 => 4,
            Self::Mat2 => 4,
            Self::Mat3 => 9,
            Self::Mat4 => 16,
        }
    }
    pub fn matrix_dim(self) -> Option<usize> {
        match self {
            Self::Mat2 => Some(2),
            Self::Mat3 => Some(3),
            Self::Mat4 => Some(4),
            _ => None,
        }
    }
    fn vector(n: usize) -> Self {
        [Self::Float, Self::Vec2, Self::Vec3, Self::Vec4][n - 1]
    }
    pub fn is_integer(self) -> bool {
        matches!(self, Self::Int | Self::IVec2 | Self::IVec3 | Self::IVec4)
    }
    fn is_vector(self) -> bool {
        matches!(self, Self::Vec2 | Self::Vec3 | Self::Vec4 | Self::IVec2 | Self::IVec3 | Self::IVec4)
    }
    fn integer_vector(n: usize) -> Self {
        [Self::Int, Self::IVec2, Self::IVec3, Self::IVec4][n - 1]
    }
    fn helper_type(self) -> bool {
        matches!(self, Self::Bool | Self::Float | Self::Int) || self.is_vector()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precision { Low, Medium, High }
impl Precision {
    fn parse(name: &str) -> Option<Self> {
        match name { "lowp" => Some(Self::Low), "mediump" => Some(Self::Medium), "highp" => Some(Self::High), _ => None }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Value {
    Floats(Ty, [f32; 16]),
    Integers(Ty, [i32; 4]),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvalError { Type, Shape, MissingReturn }
pub(crate) type EvalResult<T> = Result<T, EvalError>;
impl Value {
    pub fn zero(ty: Ty) -> Self {
        if ty.is_integer() { Self::Integers(ty, [0; 4]) } else { Self::Floats(ty, [0.; 16]) }
    }
    pub fn ty(&self) -> Ty {
        match self { Self::Floats(ty, _) | Self::Integers(ty, _) => *ty }
    }
    pub fn floats(&self) -> EvalResult<&[f32; 16]> {
        match self { Self::Floats(_, values) => Ok(values), Self::Integers(_, _) => Err(EvalError::Type) }
    }
    pub fn floats_mut(&mut self) -> EvalResult<&mut [f32; 16]> {
        match self { Self::Floats(_, values) => Ok(values), Self::Integers(_, _) => Err(EvalError::Type) }
    }
    pub fn integers(&self) -> EvalResult<&[i32; 4]> {
        match self { Self::Integers(_, values) => Ok(values), Self::Floats(_, _) => Err(EvalError::Type) }
    }
    pub fn from(ty: Ty, src: &[f32]) -> EvalResult<Self> {
        if ty.is_integer() { return Err(EvalError::Type); }
        if src.len() != ty.n() { return Err(EvalError::Shape); }
        let mut values = [0.; 16];
        values[..src.len()].copy_from_slice(src);
        Ok(Self::Floats(ty, values))
    }
    pub fn from_i32(ty: Ty, src: &[i32]) -> EvalResult<Self> {
        if !ty.is_integer() { return Err(EvalError::Type); }
        if src.len() != ty.n() { return Err(EvalError::Shape); }
        let mut values = [0; 4];
        values[..src.len()].copy_from_slice(src);
        Ok(Self::Integers(ty, values))
    }
    fn validate(&self, expected: Ty) -> EvalResult<()> {
        if self.ty() != expected { return Err(EvalError::Type); }
        match self {
            Self::Integers(ty, _) if ty.is_integer() => Ok(()),
            Self::Floats(ty, _) if !ty.is_integer() => Ok(()),
            _ => Err(EvalError::Type),
        }
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
    Integer(i32),
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
fn execute(statements: &[Statement], env: &mut BTreeMap<String,Value>, globals: &BTreeMap<String,Value>, sample: &dyn Fn(i32,[f32;2])->EvalResult<[f32;4]>) -> EvalResult<Flow> {
    for statement in statements {
        let flow = match statement {
            Statement::Assign(name, expr) => {
                let value = expr.eval(env, globals, sample)?;
                env.insert(name.clone(), value);
                Flow::Next
            }
            Statement::Block(body) => execute(body,env,globals,sample)?,
            Statement::If(condition, yes, no) => execute(if condition.eval(env,globals,sample)?.floats()?[0] != 0. { yes } else { no }, env,globals,sample)?,
            Statement::Return(expr) => Flow::Return(expr.as_ref().map(|expr| expr.eval(env,globals,sample)).transpose()?),
        };
        if !matches!(flow, Flow::Next) { return Ok(flow); }
    }
    Ok(Flow::Next)
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
        sample: &dyn Fn(i32, [f32; 2]) -> EvalResult<[f32; 4]>,
    ) -> EvalResult<Value> {
        if values.len() != self.parameters.len() { return Err(EvalError::Shape); }
        let mut env = BTreeMap::new();
        for (name, symbol) in &self.locals {
            env.insert(name.clone(), Value::zero(symbol.ty));
        }
        for ((name, ty), value) in self.parameters.iter().zip(values) {
            value.validate(*ty)?;
            env.insert(name.clone(), *value);
        }
        match execute(&self.statements, &mut env, globals, sample)? {
            Flow::Return(Some(value)) => Ok(value),
            _ => Err(EvalError::MissingReturn),
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
    fn eval(self, ty: Ty, args: &[Value]) -> EvalResult<Value> {
        let a = args.first().ok_or(EvalError::Shape)?;
        if args.len() > 3 { return Err(EvalError::Shape); }
        let mut input = [&[0.; 16]; 3];
        for (i, arg) in args.iter().enumerate() { input[i] = arg.floats()?; }
        let mut result = Value::zero(ty);
        let output = result.floats_mut()?;
        match self {
            Self::Length | Self::Distance | Self::Dot => {
                output[0] = (0..a.ty().n())
                    .map(|i| match self {
                        Self::Dot => input[0][i] * input[1][i],
                        Self::Distance => (input[0][i] - input[1][i]).powi(2),
                        _ => input[0][i] * input[0][i],
                    })
                    .sum::<f32>();
                if !matches!(self, Self::Dot) {
                    output[0] = output[0].sqrt();
                }
            }
            Self::Normalize => {
                let length = input[0][..a.ty().n()].iter().map(|x| x * x).sum::<f32>().sqrt();
                for i in 0..a.ty().n() {
                    output[i] = input[0][i] / length;
                }
            }
            Self::Cross => {
                output[..3].copy_from_slice(&[
                    input[0][1] * input[1][2] - input[0][2] * input[1][1],
                    input[0][2] * input[1][0] - input[0][0] * input[1][2],
                    input[0][0] * input[1][1] - input[0][1] * input[1][0],
                ]);
            }
            _ => {
                for i in 0..ty.n() {
                    let at =
                        |arg: usize| input[arg][if args[arg].ty() == Ty::Float { 0 } else { i }];
                    let x = at(0);
                    output[i] = match self {
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
        Ok(result)
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Shader {
    pub version: u32,
    pub output: String,
    pub locations: BTreeMap<String, u32>,
    pub symbols: BTreeMap<String, Symbol>,
    pub integer_uniform_precisions: BTreeMap<String, Precision>,
    pub reads: BTreeSet<String>,
    pub writes: BTreeSet<String>,
    function_globals: BTreeSet<String>,
    statements: Vec<Statement>,
    pub cost: usize,
}
impl Shader {
    /// Uniforms share program storage, but other names remain stage-local. A
    /// uniform from the other stage must not initialize this stage's attribute,
    /// varying or output when their legal names happen to be equal.
    pub fn uniform_inputs(&self, program: &BTreeMap<String, Value>) -> EvalResult<BTreeMap<String, Value>> {
        let mut env = BTreeMap::new();
        for (name, symbol) in &self.symbols {
            if symbol.qual != Qual::Uniform { continue; }
            if let Some(value) = program.get(name) {
                value.validate(symbol.ty)?;
                env.insert(name.clone(), *value);
            } else if self.reads.contains(name) { return Err(EvalError::Type); }
        }
        Ok(env)
    }
    /// Values supplied by the context must match both the declared type and the
    /// payload variant. Missing locals are initialized by run_with_sampler.
    pub fn validate_inputs(&self, env: &BTreeMap<String, Value>) -> EvalResult<()> {
        for (name, symbol) in &self.symbols {
            if let Some(value) = env.get(name) { value.validate(symbol.ty)?; }
            else if symbol.qual == Qual::Uniform && self.reads.contains(name) { return Err(EvalError::Type); }
        }
        Ok(())
    }
    fn validate_program(&self) -> EvalResult<()> {
        for (name, symbol) in &self.symbols {
            if symbol.qual == Qual::Uniform && symbol.ty.is_integer() && !self.integer_uniform_precisions.contains_key(name) { return Err(EvalError::Type); }
        }
        if self.symbols.get(&self.output).is_none_or(|symbol| symbol.ty != Ty::Vec4) { return Err(EvalError::Type); }
        let globals: BTreeMap<_, _> = self.function_globals.iter().map(|name| {
            let symbol = self.symbols.get(name).ok_or(EvalError::Type)?;
            if symbol.qual == Qual::Local { return Err(EvalError::Type); }
            Ok((name.clone(), symbol.ty))
        }).collect::<EvalResult<_>>()?;
        let locals: BTreeMap<_, _> = self.symbols.iter().map(|(name,s)| (name.clone(),s.ty)).collect();
        let mut budget = 4096;
        validate_statements(&self.statements, &locals, &globals, None, &mut budget)
    }
    pub fn run(&self, env: BTreeMap<String, Value>) -> EvalResult<BTreeMap<String, Value>> {
        self.run_with_sampler(env, &|_, _| {
            Err(EvalError::Type)
        })
    }
    pub fn run_with_sampler(
        &self,
        mut env: BTreeMap<String, Value>,
        sample: &dyn Fn(i32, [f32; 2]) -> EvalResult<[f32; 4]>,
    ) -> EvalResult<BTreeMap<String, Value>> {
        self.validate_inputs(&env)?;
        for (name, sym) in &self.symbols {
            env.entry(name.clone())
                .or_insert_with(|| Value::zero(sym.ty));
        }
        // Ordinary shaders keep the zero-allocation empty map. Helpers capture
        // only immutable globals they actually read, not the caller's locals.
        let globals = self
            .function_globals
            .iter()
            .map(|name| Ok((name.clone(), *env.get(name).ok_or(EvalError::Type)?)))
            .collect::<EvalResult<_>>()?;
        execute(&self.statements, &mut env, &globals, sample)?;
        Ok(env)
    }
}
fn validation_step(budget: &mut usize) -> EvalResult<()> {
    *budget = budget.checked_sub(1).ok_or(EvalError::Shape)?;
    Ok(())
}
fn constructor_valid(ty: Ty, args: &[Expr]) -> bool {
    if args.is_empty() || args.len() > 16 || ty == Ty::Sampler2D { return false; }
    if ty == Ty::Bool { return args.len() == 1 && args[0].ty == Ty::Bool; }
    if args.iter().any(|arg| matches!(arg.ty, Ty::Bool | Ty::Sampler2D)) { return false; }
    if ty.is_integer() && args.iter().any(|arg| !arg.ty.is_integer()) { return false; }
    if ty == Ty::Int { return args.len() == 1 && args[0].ty == Ty::Int; }
    let scalar = args.len() == 1 && matches!(args[0].ty, Ty::Float | Ty::Int);
    if ty == Ty::Float { return scalar; }
    let total: usize = args.iter().map(|arg| arg.ty.n()).sum();
    let conversion = ty.matrix_dim().is_some() && args.len() == 1 && args[0].ty.matrix_dim().is_some();
    let trunc = args.len() == 1 && args[0].ty.matrix_dim().is_none() && total >= ty.n() && ty.matrix_dim().is_none();
    let matrix_components = ty.matrix_dim().is_some() && total >= ty.n() && total - args[args.len()-1].ty.n() < ty.n();
    (scalar || conversion || trunc || matrix_components || total == ty.n())
        && (conversion || args.iter().all(|arg| arg.ty.matrix_dim().is_none()))
}
fn constructor_float_component(value: &Value, index: usize) -> EvalResult<f32> {
    if value.ty().is_integer() { Ok(value.integers()?[index] as f32) }
    else { Ok(value.floats()?[index]) }
}
fn validate_statements(body: &[Statement], locals: &BTreeMap<String, Ty>, globals: &BTreeMap<String, Ty>, result: Option<Ty>, budget: &mut usize) -> EvalResult<()> {
    for statement in body {
        validation_step(budget)?;
        match statement {
            Statement::Assign(name, expr) => {
                expr.validate(locals, globals, budget)?;
                if locals.get(name) != Some(&expr.ty) { return Err(EvalError::Type); }
            }
            Statement::Block(body) => validate_statements(body, locals, globals, result, budget)?,
            Statement::If(condition, yes, no) => {
                condition.validate(locals, globals, budget)?;
                if condition.ty != Ty::Bool { return Err(EvalError::Type); }
                validate_statements(yes, locals, globals, result, budget)?;
                validate_statements(no, locals, globals, result, budget)?;
            }
            Statement::Return(expr) => match (result, expr) {
                (Some(expected), Some(expr)) => {
                    expr.validate(locals, globals, budget)?;
                    if expected != expr.ty { return Err(EvalError::Type); }
                }
                (None, None) => {}
                _ => return Err(EvalError::Type),
            }
        }
    }
    Ok(())
}
impl Function {
    fn validate(&self, globals: &BTreeMap<String, Ty>, budget: &mut usize) -> EvalResult<()> {
        if self.parameters.len() > MAX_PARAMETERS || self.call_depth > MAX_CALL_DEPTH { return Err(EvalError::Shape); }
        let locals: BTreeMap<_, _> = self.locals.iter().map(|(name,s)| (name.clone(),s.ty)).collect();
        for (name, ty) in &self.parameters {
            if locals.get(name) != Some(ty) { return Err(EvalError::Type); }
        }
        if !self.statements.last().is_some_and(Statement::returns) { return Err(EvalError::MissingReturn); }
        let captures = self.globals.iter().map(|name| Ok((name.clone(), *globals.get(name).ok_or(EvalError::Type)?))).collect::<EvalResult<BTreeMap<_, _>>>()?;
        validate_statements(&self.statements, &locals, &captures, Some(self.result_type), budget)
    }
}
impl Expr {
    fn validate(&self, locals: &BTreeMap<String, Ty>, globals: &BTreeMap<String, Ty>, budget: &mut usize) -> EvalResult<()> {
        validation_step(budget)?;
        let valid = match &self.kind {
            ExprKind::Number(_) => self.ty == Ty::Float,
            ExprKind::Integer(_) => self.ty == Ty::Int,
            ExprKind::Boolean(_) => self.ty == Ty::Bool,
            ExprKind::Name(name) => locals.get(name) == Some(&self.ty),
            ExprKind::Global(name) => globals.get(name) == Some(&self.ty),
            ExprKind::Neg(expr) | ExprKind::Not(expr) => {
                expr.validate(locals, globals, budget)?;
                if matches!(self.kind, ExprKind::Not(_)) { self.ty == Ty::Bool && expr.ty == Ty::Bool }
                else { self.ty == expr.ty && !matches!(self.ty, Ty::Bool | Ty::Sampler2D) }
            }
            ExprKind::Logical(_, a, b) => {
                a.validate(locals, globals, budget)?; b.validate(locals, globals, budget)?;
                self.ty == Ty::Bool && a.ty == Ty::Bool && b.ty == Ty::Bool
            }
            ExprKind::Compare(op, a, b) => {
                a.validate(locals, globals, budget)?; b.validate(locals, globals, budget)?;
                self.ty == Ty::Bool && a.ty == b.ty && if matches!(op, Compare::Equal | Compare::NotEqual) {
                    a.ty != Ty::Sampler2D
                } else { matches!(a.ty, Ty::Float | Ty::Int) }
            }
            ExprKind::Swizzle(expr, indices) => {
                expr.validate(locals, globals, budget)?;
                !indices.is_empty() && indices.len() <= 4 && expr.ty.is_vector()
                    && indices.iter().all(|i| *i < expr.ty.n()) && self.ty == if expr.ty.is_integer() { Ty::integer_vector(indices.len()) } else { Ty::vector(indices.len()) }
            }
            ExprKind::Texture(unit, uv) => {
                unit.validate(locals, globals, budget)?; uv.validate(locals, globals, budget)?;
                self.ty == Ty::Vec4 && unit.ty == Ty::Sampler2D && uv.ty == Ty::Vec2
            }
            ExprKind::Builtin(function, args) => {
                for arg in args { arg.validate(locals, globals, budget)?; }
                function.result_type(args).is_ok_and(|ty| ty == self.ty)
            }
            ExprKind::Call(function, args) => {
                if args.len() != function.parameters.len() { return Err(EvalError::Shape); }
                for (arg, (_, ty)) in args.iter().zip(&function.parameters) {
                    arg.validate(locals, globals, budget)?;
                    if arg.ty != *ty { return Err(EvalError::Type); }
                }
                function.validate(globals, budget)?;
                self.ty == function.result_type
            }
            ExprKind::Construct(args) => {
                if args.is_empty() || args.len() > 16 { return Err(EvalError::Shape); }
                for arg in args { arg.validate(locals, globals, budget)?; }
                constructor_valid(self.ty, args)
            }
            ExprKind::Binary(op, a, b) => {
                a.validate(locals, globals, budget)?; b.validate(locals, globals, budget)?;
                if !matches!(op, '+' | '-' | '*' | '/') || matches!(a.ty, Ty::Bool | Ty::Sampler2D) || matches!(b.ty, Ty::Bool | Ty::Sampler2D) || a.ty.is_integer() || b.ty.is_integer() { return Err(EvalError::Type); }
                let (ad, bd) = (a.ty.matrix_dim(), b.ty.matrix_dim());
                let expected = if *op == '*' && ad.is_some_and(|n| b.ty == Ty::vector(n)) { Some(b.ty) }
                    else if *op == '*' && bd.is_some_and(|n| a.ty == Ty::vector(n)) { Some(a.ty) }
                    else if a.ty == b.ty || b.ty == Ty::Float { Some(a.ty) }
                    else if a.ty == Ty::Float { Some(b.ty) } else { None };
                expected == Some(self.ty)
            }
        };
        if valid { Ok(()) } else { Err(EvalError::Type) }
    }
    fn eval(
        &self,
        env: &BTreeMap<String, Value>,
        globals: &BTreeMap<String, Value>,
        sample: &dyn Fn(i32, [f32; 2]) -> EvalResult<[f32; 4]>,
    ) -> EvalResult<Value> {
        match &self.kind {
            ExprKind::Boolean(value) => Value::from(Ty::Bool, &[u8::from(*value) as f32]),
            ExprKind::Not(expr) => Value::from(Ty::Bool, &[u8::from(expr.eval(env,globals,sample)?.floats()?[0] == 0.) as f32]),
            ExprKind::Logical(op,a,b) => {
                let a = a.eval(env,globals,sample)?.floats()?[0] != 0.;
                let value = match op {
                    Logical::And => a && b.eval(env,globals,sample)?.floats()?[0] != 0.,
                    Logical::Or => a || b.eval(env,globals,sample)?.floats()?[0] != 0.,
                    Logical::Xor => a != (b.eval(env,globals,sample)?.floats()?[0] != 0.),
                };
                Value::from(Ty::Bool,&[u8::from(value) as f32])
            }
            ExprKind::Compare(op,a,b) => {
                let a=a.eval(env,globals,sample)?;
                let b=b.eval(env,globals,sample)?;
                let value = if a.ty().is_integer() {
                    let av = a.integers()?; let bv = b.integers()?;
                    match op {
                        Compare::Less => av[0] < bv[0], Compare::LessEqual => av[0] <= bv[0],
                        Compare::Greater => av[0] > bv[0], Compare::GreaterEqual => av[0] >= bv[0],
                        Compare::Equal => av[..a.ty().n()] == bv[..b.ty().n()],
                        Compare::NotEqual => av[..a.ty().n()] != bv[..b.ty().n()],
                    }
                } else {
                    let av = a.floats()?; let bv = b.floats()?;
                    match op {
                        Compare::Less => av[0] < bv[0], Compare::LessEqual => av[0] <= bv[0],
                        Compare::Greater => av[0] > bv[0], Compare::GreaterEqual => av[0] >= bv[0],
                        Compare::Equal => av[..a.ty().n()] == bv[..b.ty().n()],
                        Compare::NotEqual => av[..a.ty().n()] != bv[..b.ty().n()],
                    }
                };
                Value::from(Ty::Bool,&[u8::from(value) as f32])
            }
            ExprKind::Call(function, args) => {
                let mut values = [Value::zero(Ty::Float); MAX_PARAMETERS];
                for (i, arg) in args.iter().enumerate() {
                    values[i] = arg.eval(env, globals, sample)?;
                }
                function.eval(globals, &values[..args.len()], sample)
            }
            ExprKind::Builtin(function, args) => {
                let mut values = [Value::zero(Ty::Float); 3];
                for (i, arg) in args.iter().enumerate() {
                    values[i] = arg.eval(env, globals, sample)?;
                }
                function.eval(self.ty, &values[..args.len()])
            }
            ExprKind::Texture(unit, uv) => {
                let unit = unit.eval(env, globals, sample)?;
                let uv = uv.eval(env, globals, sample)?;
                Value::from(Ty::Vec4, &sample(unit.floats()?[0] as i32, [uv.floats()?[0], uv.floats()?[1]])?)
            }
            ExprKind::Number(n) => Value::from(self.ty, &[*n]),
            ExprKind::Integer(n) => Value::from_i32(Ty::Int, &[*n]),
            ExprKind::Name(n) => env.get(n).copied().ok_or(EvalError::Type),
            ExprKind::Global(n) => globals.get(n).copied().ok_or(EvalError::Type),
            ExprKind::Neg(e) => {
                let mut r = e.eval(env, globals, sample)?;
                let n = r.ty().n();
                if r.ty().is_integer() {
                    let values = (*r.integers()?).map(i32::wrapping_neg);
                    return Value::from_i32(r.ty(), &values[..n]);
                }
                for v in &mut r.floats_mut()?[..n] {
                    *v = -*v;
                }
                Ok(r)
            }
            ExprKind::Swizzle(e, ix) => {
                let a = e.eval(env, globals, sample)?;
                if self.ty.is_integer() {
                    let mut values = [0; 4];
                    for (i, &j) in ix.iter().enumerate() { values[i] = a.integers()?[j]; }
                    return Value::from_i32(self.ty, &values[..self.ty.n()]);
                }
                let mut r = Value::zero(self.ty);
                for (i, &j) in ix.iter().enumerate() {
                    r.floats_mut()?[i] = a.floats()?[j];
                }
                Ok(r)
            }
            ExprKind::Construct(args) => {
                let values: Vec<_> = args.iter().map(|a| a.eval(env, globals, sample)).collect::<EvalResult<_>>()?;
                if values.is_empty() { return Err(EvalError::Shape); }
                if self.ty.is_integer() {
                    let mut output = [0; 4];
                    if values.len() == 1 && values[0].ty() == Ty::Int {
                        output[..self.ty.n()].fill(values[0].integers()?[0]);
                    } else {
                        let mut i = 0;
                        for value in values {
                            for j in 0..value.ty().n() {
                                if i < self.ty.n() { output[i] = value.integers()?[j]; i += 1; }
                            }
                        }
                    }
                    return Value::from_i32(self.ty, &output[..self.ty.n()]);
                }
                let mut r = Value::zero(self.ty);
                let output = r.floats_mut()?;
                if values.len() == 1 && matches!(values[0].ty(), Ty::Float | Ty::Int) {
                    if let Some(n) = self.ty.matrix_dim() {
                        for i in 0..n {
                            output[i * (n + 1)] = constructor_float_component(&values[0], 0)?;
                        }
                    } else {
                        output[..self.ty.n()].fill(constructor_float_component(&values[0], 0)?);
                    }
                } else if let (Some(dst), Some(src)) = (self.ty.matrix_dim(), values[0].ty().matrix_dim()) {
                    // Matrix constructors preserve the common upper-left block;
                    // components outside it come from the identity matrix.
                    for col in 0..dst {
                        for row in 0..dst {
                            output[col * dst + row] = if col < src && row < src {
                                values[0].floats()?[col * src + row]
                            } else { u8::from(col == row) as f32 };
                        }
                    }
                } else {
                    let mut i = 0;
                    for v in values {
                        for j in 0..v.ty().n() {
                            if i < self.ty.n() {
                                output[i] = constructor_float_component(&v, j)?;
                                i += 1;
                            }
                        }
                    }
                }
                Ok(r)
            }
            ExprKind::Binary(op, a, b) => {
                let (a, b) = (a.eval(env, globals, sample)?, b.eval(env, globals, sample)?);
                let av = a.floats()?;
                let bv = b.floats()?;
                let mut r = Value::zero(self.ty);
                let output = r.floats_mut()?;
                let (ad, bd) = (a.ty().matrix_dim(), b.ty().matrix_dim());
                if *op == '*' && ad.is_some() && bd.is_none() && b.ty() != Ty::Float {
                    let n = ad.unwrap();
                    for row in 0..n {
                        output[row] = (0..n).map(|col| av[col * n + row] * bv[col]).sum();
                    }
                } else if *op == '*' && bd.is_some() && ad.is_none() && a.ty() != Ty::Float {
                    let n = bd.unwrap();
                    for col in 0..n {
                        output[col] = (0..n).map(|row| av[row] * bv[col * n + row]).sum();
                    }
                } else if *op == '*' && ad.is_some() && bd.is_some() {
                    let n = ad.unwrap();
                    for col in 0..n {
                        for row in 0..n {
                            output[col * n + row] =
                                (0..n).map(|k| av[k * n + row] * bv[col * n + k]).sum();
                        }
                    }
                } else {
                    for i in 0..self.ty.n() {
                        let x = av[if a.ty() == Ty::Float { 0 } else { i }];
                        let y = bv[if b.ty() == Ty::Float { 0 } else { i }];
                        output[i] = match op {
                            '+' => x + y,
                            '-' => x - y,
                            '*' => x * y,
                            '/' => x / y,
                            _ => unreachable!(),
                        };
                    }
                }
                Ok(r)
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Id(String),
    Num(f32),
    Integer(u32),
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
            let integral = !src[s..i].contains(['.', 'e', 'E']);
            if integral {
                if i - s > 1 && b[s] == b'0' { return Err("octal integer literals are unsupported".into()); }
                let bits: u32 = src[s..i].parse().map_err(|_| "integer literal exceeds 32 bits")?;
                out.push(Tok::Integer(bits));
            } else {
                let number: f32 = src[s..i].parse().map_err(|_| "invalid numeric literal")?;
                if !number.is_finite() { return Err("numeric literal is not finite".into()); }
                out.push(Tok::Num(number));
            }
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
    default_int_precision: Precision,
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
    fn ty(&mut self) -> Result<(Ty, Option<Precision>), String> {
        let mut name = self.id()?;
        let precision = Precision::parse(&name);
        if precision.is_some() { name = self.id()?; }
        let ty = Ty::parse(&name).ok_or_else(|| format!("unsupported type {name}"))?;
        if precision.is_some() && ty == Ty::Bool { return Err("bool cannot have a precision qualifier".into()); }
        Ok((ty, precision))
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
            let (ty, _) = self.ty()?;
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
                let (ty, _) = self.ty()?;
                if !ty.helper_type() {
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
            Tok::Num(n) => Expr { ty: Ty::Float, kind: ExprKind::Number(n) },
            Tok::Integer(bits) => Expr { ty: Ty::Int, kind: ExprKind::Integer(bits as i32) },
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
                    if !constructor_valid(ty, &args) {
                        return Err("unsupported constructor types or component count; float-to-int and bool conversions are unsupported".into());
                    }
                    if ty.matrix_dim().is_some() {
                        self.shader.cost += ty.n();
                        if self.shader.cost > 512 {
                            return Err("shader exceeds expression budget".into());
                        }
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
                if !left.ty.is_vector() {
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
                    ty: if left.ty.is_integer() { Ty::integer_vector(ix.len()) } else { Ty::vector(ix.len()) },
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
                    if left.ty != right.ty || left.ty == Ty::Sampler2D {
                        return Err("equality requires matching bool, float, vector or matrix operands".into());
                    }
                } else if left.ty != right.ty || !matches!(left.ty, Ty::Float | Ty::Int) {
                    return Err("relational comparisons require matching scalar int or float operands".into());
                }
                let op=match op.as_str() {"<"=>Compare::Less,"<="=>Compare::LessEqual,">"=>Compare::Greater,">="=>Compare::GreaterEqual,"=="=>Compare::Equal,_=>Compare::NotEqual};
                ty=Ty::Bool;
                kind=ExprKind::Compare(op,Box::new(left),Box::new(right));
            } else {
                if matches!(left.ty,Ty::Sampler2D|Ty::Bool) || matches!(right.ty,Ty::Sampler2D|Ty::Bool) {
                    return Err("arithmetic requires numeric operands".into());
                }
                if left.ty.is_integer() || right.ty.is_integer() {
                    return Err("binary integer arithmetic and implicit integer-to-float conversion are unsupported".into());
                }
                let (ad, bd) = (left.ty.matrix_dim(), right.ty.matrix_dim());
                ty = if op == "*" && ad.is_some_and(|n| right.ty == Ty::vector(n)) {right.ty}
                    else if op == "*" && bd.is_some_and(|n| left.ty == Ty::vector(n)) {left.ty}
                    else if left.ty==right.ty {left.ty}
                    else if left.ty==Ty::Float {right.ty}
                    else if right.ty==Ty::Float {left.ty}
                    else {return Err("binary operand types do not match".into());};
                if let Some(n) = ad.or(bd) {
                    // Matrix products do cubic work; matrix/vector products
                    // and component operations do quadratic work. Every call
                    // site, including helper bodies, retains this charge.
                    self.shader.cost += if op == "*" && ad.is_some() && bd.is_some() { n*n*n } else { n*n };
                    if self.shader.cost > 512 {
                        return Err("shader exceeds expression budget".into());
                    }
                }
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
    let source = crate::preprocessor::preprocess(source, token_limit)?;
    let (tokens, version) = lex(&source, token_limit)?;
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
            integer_uniform_precisions: BTreeMap::new(),
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
        default_int_precision: if vertex { Precision::High } else { Precision::Medium },
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
                Tok::Integer(n) if n < crate::MAX_VERTEX_ATTRIBS => n,
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
                "int" => p.default_int_precision = Precision::parse(&q).ok_or("invalid integer precision")?,
                "sampler2D" => {}
                _ => return Err("unsupported precision declaration type".into()),
            }
            p.expect(Tok::P(';'))?;
            continue;
        }
        if matches!(
            n.as_str(),
            "bool" | "float" | "vec2" | "vec3" | "vec4" | "int" | "ivec2" | "ivec3" | "ivec4" | "lowp" | "mediump" | "highp"
        ) {
            if location.is_some() {
                return Err("layout cannot qualify a user function".into());
            }
            p.pos -= 1;
            let (result_type, _) = p.ty()?;
            if !result_type.helper_type() {
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
        let (ty, precision) = p.ty()?;
        if ty.is_integer() && qual != Qual::Uniform { return Err("integer interfaces other than uniforms are unsupported".into()); }
        if ty == Ty::Bool && qual != Qual::Uniform {
            return Err("bool interfaces other than uniforms are unsupported".into());
        }
        if ty.matrix_dim().is_some() && qual != Qual::Uniform {
            return Err("matrices are supported only for uniforms and locals".into());
        }
        if qual == Qual::Output && ty != Ty::Vec4 {
            return Err("only a vec4 fragment output is supported".into());
        }
        let name = p.id()?;
        p.declare(name.clone(), ty, qual)?;
        if ty.is_integer() && qual == Qual::Uniform { p.shader.integer_uniform_precisions.insert(name.clone(), precision.unwrap_or(p.default_int_precision)); }
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
    p.shader.validate_program().map_err(|error| format!("shader invariant validation failed: {error:?}"))?;
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
                let env = BTreeMap::from([("image".into(), Value::zero(Ty::Sampler2D))]);
                let result=shader.run_with_sampler(env,&|_,_| {calls.set(calls.get()+1);Ok([1.,0.,0.,1.])}).unwrap();
                assert_eq!(calls.get(),expected_calls,"{version}: {expression}");
                assert_eq!(result[output].floats().unwrap()[0],if expected_red {1.}else{0.},"{version}: {expression}");
            }
        }
    }
}

#[cfg(test)]
mod typed_value_contract {
    use super::*;
    use std::cell::Cell;
    use std::mem::size_of;

    #[test]
    fn value_storage_does_not_exceed_the_previous_float_layout() {
        #[allow(dead_code)]
        struct PreviousValue { ty: Ty, values: [f32; 16] }
        assert!(size_of::<Value>() <= size_of::<PreviousValue>(), "new={} previous={}", size_of::<Value>(), size_of::<PreviousValue>());
    }

    #[test]
    fn typed_payloads_reject_wrong_views_tags_and_component_counts() {
        let source = [i32::MIN, i32::MAX, 16_777_217, -16_777_217];
        let value = Value::from_i32(Ty::IVec4, &source).unwrap();
        assert_eq!(value.integers().unwrap(), &source);
        assert_eq!(value.floats().unwrap_err(), EvalError::Type);
        assert_eq!(Value::from(Ty::Int, &[1.]).unwrap_err(), EvalError::Type);
        assert_eq!(Value::from_i32(Ty::Float, &[1]).unwrap_err(), EvalError::Type);
        assert_eq!(Value::from_i32(Ty::IVec3, &[1, 2]).unwrap_err(), EvalError::Shape);
        assert_eq!(Value::from(Ty::Vec4, &[1., 2., 3.]).unwrap_err(), EvalError::Shape);
        assert_eq!(Value::Integers(Ty::Float, [0; 4]).validate(Ty::Float), Err(EvalError::Type));
        assert_eq!(Value::Floats(Ty::Int, [0.; 16]).validate(Ty::Int), Err(EvalError::Type));
    }

    #[test]
    fn float_payload_copies_preserve_nan_infinity_and_negative_zero_bits() {
        let source = [f32::from_bits(0x7fc01234), f32::INFINITY, f32::NEG_INFINITY, -0.];
        let value = Value::from(Ty::Vec4, &source).unwrap();
        assert_eq!(value.floats().unwrap()[..4].iter().map(|v| v.to_bits()).collect::<Vec<_>>(), source.map(f32::to_bits));
        assert_eq!(value.integers().unwrap_err(), EvalError::Type);
    }

    fn source() -> &'static str {
        "precision mediump float;uniform sampler2D image;vec4 inner(){return texture2D(image,vec2(0.5));}vec4 outer(){return inner();}void main(){if(false){gl_FragColor=outer();}else{gl_FragColor=vec4(0.0,1.0,0.0,1.0);}}"
    }
    fn branch_expr(shader: &mut Shader) -> &mut Expr {
        let Statement::If(_, yes, _) = &mut shader.statements[0] else { panic!("fixture if"); };
        let Statement::Assign(_, expr) = &mut yes[0] else { panic!("fixture assignment"); };
        expr
    }
    fn inner_function(shader: &mut Shader) -> &mut Function {
        let ExprKind::Call(outer, _) = &mut branch_expr(shader).kind else { panic!("fixture outer call"); };
        let Statement::Return(Some(expr)) = &mut Arc::make_mut(outer).statements[0] else { panic!("fixture outer return"); };
        let ExprKind::Call(inner, _) = &mut expr.kind else { panic!("fixture inner call"); };
        Arc::make_mut(inner)
    }

    #[test]
    fn invariant_validation_checks_untaken_branches_and_transitive_helpers_before_sampling() {
        let original = compile(source(), false, 100).unwrap();
        for case in 0..4 {
            let mut invalid = original.clone();
            let expected = match case {
                0 => {
                    let ExprKind::Call(_, args) = &mut branch_expr(&mut invalid).kind else { panic!("fixture call"); };
                    args.push(Expr { ty: Ty::Float, kind: ExprKind::Number(1.) });
                    EvalError::Shape
                }
                1 => { inner_function(&mut invalid).statements.clear(); EvalError::MissingReturn }
                2 => {
                    let original = branch_expr(&mut invalid).clone();
                    branch_expr(&mut invalid).kind = ExprKind::Swizzle(Box::new(original), vec![4, 0, 1, 2]);
                    EvalError::Type
                }
                _ => { branch_expr(&mut invalid).kind = ExprKind::Construct(vec![]); EvalError::Shape }
            };
            let calls = Cell::new(0);
            let outcome = (|| {
                invalid.validate_program()?;
                invalid.run_with_sampler(BTreeMap::from([("image".into(), Value::zero(Ty::Sampler2D))]), &|_, _| { calls.set(calls.get()+1); Ok([1.,0.,0.,1.]) })
            })();
            assert_eq!(outcome.unwrap_err(), expected, "case={case}");
            assert_eq!(calls.get(), 0, "case={case}");
        }
        let valid = compile(&source().replace("if(false)", "if(true)"), false, 100).unwrap();
        let calls = Cell::new(0);
        let result = valid.run_with_sampler(BTreeMap::from([("image".into(), Value::zero(Ty::Sampler2D))]), &|_, _| { calls.set(calls.get()+1); Ok([1.,0.,0.,1.]) }).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(&result["gl_FragColor"].floats().unwrap()[..4], &[1.,0.,0.,1.]);
    }

    #[test]
    fn vertex_entry_rejects_texture_execution_instead_of_panicking_or_faking_a_texel() {
        let shader = compile(&source().replace("if(false)", "if(true)"), false, 100).unwrap();
        assert_eq!(shader.run(BTreeMap::from([("image".into(), Value::zero(Ty::Sampler2D))])).unwrap_err(), EvalError::Type);
    }

    #[test]
    fn integer_interfaces_outside_uniforms_are_still_rejected() {
        for version in [100, 300] {
            for ty in ["int", "ivec2", "ivec3", "ivec4"] {
                let prefix = if version == 300 { "#version 300 es\nprecision mediump float;out vec4 pixel;" } else { "precision mediump float;" };
                let output = if version == 300 { "pixel" } else { "gl_FragColor" };
                let qualifier = if version == 300 { "in" } else { "varying" };
                let shader = format!("{prefix}{qualifier} {ty} value;void main(){{{output}=vec4(1.0);}}");
                assert!(compile(&shader, false, version).is_err(), "{shader}");
                let vertex = format!("{}{} {ty} value;void main(){{gl_Position=vec4(1.0);}}", if version == 300 { "#version 300 es\n" } else { "" }, if version == 300 { "in" } else { "attribute" });
                assert!(compile(&vertex, true, version).is_err(), "{vertex}");
            }
        }
    }
}
