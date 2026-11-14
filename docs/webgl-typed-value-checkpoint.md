# Typed shader values: storage checkpoint

This isolated checkpoint follows `b88c8ff7390fa687f3a74771b32fd6057c16be3f`
and the three privately replayed uniform-validation changes. It does not enable
`int` or `ivec` shader syntax, new setters, or integer interfaces. Those remain
compiler errors or explicit unsupported operations until the later milestone.

`Value` is an enum with float and integer payload variants. The type tag is
inside each variant; it is not duplicated outside a payload enum. Integer
storage has four `i32` lanes; the existing float/matrix payload has sixteen
`f32` lanes. A unit gate compares `size_of::<Value>()` to a struct with the old
`Ty` plus `[f32;16]` layout. The candidate has not yet been built, so no measured
layout size or performance result is claimed.

Typed accessors and constructors return `EvalError` for incompatible payloads
or component counts. The evaluator propagates errors through expressions,
helpers, statements and shader entry points. Context methods map these to
`INVALID_OPERATION`. A vertex entry that encounters texture execution returns
an error; it neither panics nor supplies a replacement texel.

The compiler checks AST invariants before admitting a shader: both selection
branches, swizzle and constructor shapes, operators, builtin overloads, helper
arity/parameters, captured globals and transitive returns. This pass does not
evaluate shaders or sample textures. It has a cumulative 4096-node work bound;
the existing parser and transitive 512-unit expression budget stay in force.

Before a draw prepares geometry or writes color/depth, context-owned uniforms
are checked against the types and payload variants of both stages. Attribute
values are constructed with checked shape while geometry is prepared, before
painting. The linked varying interface and compiler AST checks constrain the
remaining generated fragment inputs to float payloads. The evaluator retains
its `Result` checks as defense; there is no second execution or framebuffer
copy used to hide a late validation error.

Uniform environments are projected per stage before either shader runs. A
fragment uniform may legally share a name with a vertex attribute, and a vertex
uniform may share a name with a fragment output. These declarations must not
initialize or be compared to another stage's non-uniform symbols. Positive
pixel controls cover both cases while malformed actual uniform payloads still
fail before any color or depth write.

The float algorithms keep their operation and matrix accumulation order.
Uniform values are not filtered for finiteness. Tests preserve NaN payload bits,
infinities and signed zero, reject same-type/wrong-variant payloads before any
color or depth write, and exercise invalid untaken branches/transitive helpers.
All nine new tests, existing backend regressions, and Script/Engine validation
remain pending root CI. No Rust tools, builds, runtime checks or SSH were used
by the implementing agent.
