# Signed32 shader/backend checkpoint

This is the second private checkpoint after typed storage `ce15131`. It enables
`int`, `ivec2/3/4` in shader uniforms, initialized locals and by-value helper
parameters/results. The JS integer-vector setters and typed query bridge are
still pending; no complete WebGL integer-vector API claim is made for this
checkpoint. It has not been compiled or executed by the implementing agent.

Integer values use the `Integers(Ty,[i32;4])` payload introduced by the first
checkpoint. Decimal literal tokens are parsed as checked 32-bit patterns before
any float conversion. Integers are never stored or compared as floats. ESSL300
signed literal patterns follow §4.1.3: `2147483648` denotes INT_MIN and
`4294967295` denotes -1. This implementation chooses signed32 storage for all
precision levels and both language versions; it does not promise portable
ESSL100 overflow behavior. Octal/hex/suffix syntax remains outside the subset.

Supported integer expressions are identity/splat/component constructors from
integers, unary signs, vector swizzles, exact scalar relational comparisons,
and scalar/vector equality. Helper scopes, captures and transitive call-site
budgets remain in force. Explicit `float(int)` and floating vector/matrix
constructors may convert integer components; that is the only float rounding
path. Implicit assignments, float-to-int/bool conversions, integer binary
arithmetic, integer math builtins and bitwise operations fail compilation.

The compiler and its invariant validator share constructor checks. Integer
interfaces other than uniforms are explicitly rejected, including WebGL2 integer
vertex attributes, flat varyings and fragment outputs. Uniform arrays,
structures, uint/uvec, bvec, builtin integer IDs and loops remain unsupported.

The backend integer setter carries method width separately from data length.
It checks width, length/count, uniform type, and sampler constraints before
writing. Float setters reject signed integer uniforms. `get_uniform_i32`
returns exact copies; legacy float queries reject integer payloads rather than
approximating them. Reflection uses INT and INT_VEC2/3/4 with size1.

Resolved declared precision is retained for integer uniforms. Explicit qualifiers
and global `precision ... int` statements override the stage defaults (highp
vertex, mediump fragment). Shared integer uniforms must match both type and
precision at link time. This deliberately does not repair the older float
precision metadata limitation. Locally scoped precision statements and new
integer precision queries in JS remain for later work or explicit rejection.

Seven new backend tests are written and pending root CI: distinct adjacent
integers above2^24 produce different real pixels; all widths preserve i32
limits and reject invalid writes atomically; integer helpers preserve lexical
bindings; integer vertex uniforms move coverage after explicit conversion;
linking checks precision/type; ESSL300 literals retain their signed bit pattern;
and excluded operations fail compile while a previous program keeps drawing.
Both ESSL100 and300 are covered where the contract is shared. Script/Engine
integer-vector coverage comes with the later binding checkpoint.
