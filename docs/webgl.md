# Experimental software WebGL1 and WebGL2

RabbitHole exposes `canvas.getContext('webgl')`, its `experimental-webgl` alias,
and an experimental `getContext('webgl2')`. They use
RabbitHole's own Rust shader evaluator, homogeneous clipper and triangle rasterizer in
`crates/webgl`. JavaScript commands produce actual drawing-buffer pixels,
present through canvas and support `readPixels`. No GPU driver, external
shader compiler or embedded browser engine is involved.

This is an initial rendering subset, not a conformant WebGL implementation.
Extensions are unavailable and Three.js compatibility has not been established.
Canvas 2D, WebGL1 and WebGL2 contexts remain mutually exclusive on a canvas.
WebGL2 has a distinct JavaScript interface and context state; selecting a shader
language version alone does not turn a WebGL1 context into WebGL2.

## Supported surface

- Shader compilation/linking with status and diagnostic logs, program
  reflection, attributes and float/vector/matrix/sampler uniforms.
- Vertex/index buffers, checked attribute pointers, normalized integer
  attributes, buffer updates and resource deletion.
- Triangles, triangle strips and fans, perspective-correct interpolation,
  clipping, depth tests, viewport, scissor and color/depth write masks.
- RGB/alpha blending with independent factors and add/subtract/reverse-subtract
  equations, constant color and source-alpha saturation. Clear bypasses blending;
  RGB-only canvases use destination alpha one.
- RGBA8 2D texture uploads and sub-updates, eight texture units and nearest or
  bilinear sampling with clamp/repeat/mirrored-repeat. DOM and typed-array uploads
  handle supported unpack flags and row alignment.
- RGBA8 render-to-texture framebuffers with optional 16-bit depth storage,
  checked attachments and feedback-loop rejection.
- WebGL2 instanced draws with real divisor attributes captured by VAOs.
- Visible canvas presentation, readback orientation and packing,
  resize behavior, context attributes and loss on oversized drawing buffers.

## Shader language subset

WebGL1 accepts a GLSL ES 1.00 subset: whole-variable assignments,
initialized local declarations, float/vector/matrix expressions, scalar bool
expressions, bounded `if`/`else` with lexical blocks, early returns and fragment
`texture2D`. Fragment shaders need an explicit float precision declaration.
Unsupported constructs fail compilation with a diagnostic. Unsupported API
operations report errors rather than pretend to render.

The evaluator implements these 32 math built-ins for its supported float/vector
overloads, with checked argument counts, types and expression budgets:

- Angles and trigonometry: `radians`, `degrees`, `sin`, `cos`, `tan`, `asin`,
  `acos`, `atan`.
- Powers and roots: `pow`, `exp`, `log`, `exp2`, `log2`, `sqrt`, `inversesqrt`.
- Common math: `abs`, `sign`, `floor`, `ceil`, `fract`, `mod`, `min`, `max`,
  `clamp`, `mix`, `step`, `smoothstep`.
- Geometry: `length`, `distance`, `dot`, `normalize`, `cross`.

User helpers support scalar `bool`, `float` and `vec2/3/4` results and up to eight named
parameters of those types, copied by value with ordinary or `in` syntax. Helpers
may execute initialized local declarations, local/parameter assignments, blocks,
selection and typed early returns. Every helper path must return; every main
exit path must write its stage output. Definitions precede callers and `main`; nested calls to earlier
helpers work in both language versions. Uniforms, attributes and fragment inputs
are read from the global environment even when a caller shadows their names.
Helpers cannot write globals or read mutable outputs.

The evaluator shares helper bodies and charges every call site, argument and
transitive body against its expression budget. There are at most 16 helpers and
16 nested calls. Recursion, forward calls, prototypes, overloading, `out`/`inout`/
`const` parameters and statements after an unconditional return are rejected. See [the backend limits](../crates/webgl/README.md#glsl-subset).
Scalar bool operations include `!`, short-circuiting `&&`/`||`, `^^`, scalar float
comparisons and equality/inequality on matching bool, float, vector or matrix
types. Both sides and all branches are checked and reflected at compile time.
Boolean interfaces other than scalar uniforms, boolean vectors, GLSL numeric/bool
conversions, loops, switch,
ternary expressions, arrays, integer arithmetic, unsupported built-in overloads
and derivatives fail compilation. The subset allows 16 nested statement levels
and 128 statements per body; skipped branches still add to the compile budget.

Scalar `uniform bool` works in vertex and fragment shaders and starts false after
linking. `uniform1i`/`uniform1iv` and `uniform1f`/`uniform1fv` set false for zero
and true for nonzero inputs, following [OpenGL ES 2.0 section 2.10.4](https://registry.khronos.org/OpenGL/specs/es/2.0/es_full_spec_2.0.pdf).
`getActiveUniform` reports `BOOL`; JavaScript `getUniform` returns a boolean.
Wrong-size/type setters preserve the value, and relinking restores false and
invalidates earlier locations. Boolean vectors and arrays remain unsupported.

## WebGL2 context milestone

WebGL2 additionally accepts the bounded GLSL ES 3.00 mode with `#version 300 es`:
vertex `in`/`out`, fragment `in`, one named `out vec4`, supported literal
`layout(location=...)` declarations and real `texture()` sampling. Legacy GLSL
1.00 sources retain their own syntax and semantics; a GLSL 3.00 source cannot use
`attribute`, `varying`, `gl_FragColor` or `texture2D()`.

Vertex array objects capture attribute pointers, enables and element-array
bindings. `ARRAY_BUFFER` and generic attribute constants remain global. Buffer
deletion preserves storage referenced by inactive VAOs until those references
are released. `UNSIGNED_INT` element indices have checked alignment and bounds;
fixed primitive restart applies to byte, short and uint indices in WebGL2 and
does not change WebGL1 index semantics. Non-power-of-two textures can repeat in
WebGL2 within the supported filtering subset.

Integer attributes and general integer uniforms, 3D textures, UBOs, queries,
sampler objects, synchronization and transform feedback remain unsupported. Extra offset upload/readback
overloads are also unsupported. Exposed unsupported operations report errors.

## Instancing through attributes

WebGL2 implements `vertexAttribDivisor`, `drawArraysInstanced` and
`drawElementsInstanced`. A nonzero divisor selects attribute data at
`floor(instance/divisor)`; a zero divisor advances per vertex. Instance data can
change actual positions, colors and other supported shader inputs. Divisors
belong to the VAO; generic constant attributes remain global. Ordinary draws
use instance zero for divisor attributes. Indexed instancing supports the same
checked index types and fixed primitive restart as other WebGL2 indexed draws.

All instance ranges, total work and prepared geometry budgets are validated
before any framebuffer pixel or depth changes. Current limits are 1024 instances,
262,144 evaluated vertex references, 32 Mi weighted work units and 64 MiB of
prepared triangle data per draw. WebGL1 does not expose instancing or its
extension. `gl_InstanceID` still fails shader compilation: integer shader types
have not been implemented, and the ID is not simulated as a float.

## Framebuffers and render-to-texture

WebGL1 and WebGL2 can render to a level-zero RGBA8 2D texture attached at
`COLOR_ATTACHMENT0`. Clear, drawing, `readPixels` and subsequent texture sampling
use actual attachment pixels. WebGL2 supports independent `READ_FRAMEBUFFER`
and `DRAW_FRAMEBUFFER` bindings; WebGL1 rejects these separate targets.

Binding an FBO preserves the viewport and other GL state. Offscreen rendering
leaves the default drawing buffer intact, and canvas presentation always uses
that default buffer. A complete instanced batch selects its target once;
target size, alpha and effective depth state are restored on success or error.
FBO textures retain their alpha channel even if the default canvas has
`alpha:false`.

An optional matching `DEPTH_COMPONENT16` renderbuffer supplies initialized,
16-bit-quantized depth storage. It works independently of the default canvas's
`depth` attribute, so an FBO depth attachment can be used with `depth:false`.
Depth-test enable state remains the requested GL state; its effect depends on
the selected target's depth storage. Clear obeys scissor and write masks.

Missing or unallocated attachments, size mismatches and texture feedback loops
are checked before drawing changes pixels. Sampling a texture attached to the
current draw FBO is rejected. Deleted attachments remain alive while referenced
by inactive FBOs; deletion detaches them from current framebuffer bindings.

This subset requires one color texture attachment and optional matching depth.
Depth-only FBOs, color renderbuffers, stencil, multiple color attachments/MRT,
blits and multisampling remain unsupported. Unsupported commands report errors;
this milestone does not establish complete WebGL2 conformance.

## Texture filtering

Set both `MIN_FILTER` and `MAG_FILTER` to `NEAREST`, or both to `LINEAR`.
`LINEAR` interpolates four texels using texel-center coordinates and the selected
wrap mode. Texture-coordinate derivatives and LOD selection are unfinished;
mixed minification/magnification filters and mipmaps are unsupported and fail
before drawing. Defaults retain the standard mipmapped/linear state, so a new
defined texture needs supported filters explicitly. In WebGL1, non-power-of-two
textures also need clamp-to-edge wrapping for completeness.

## Current limits

Stencil, culling, points/lines, multiple color attachments/MRT, blits,
multisampling, cube maps, mipmaps and compressed textures remain unsupported. GLSL loops,
switch, ternary expressions, arrays, integer variables, derivatives and discard are unavailable. Context creation does not imply that arbitrary
WebGL applications can run.

Drawing-buffer dimensions are at most 4096 per axis and 4,194,304 pixels in
total. The backend caps vertex attributes at 16, texture units at eight,
buffer storage at 64 MiB and texture storage at a separate 64 MiB.
Renderbuffers have another 64 MiB budget, and framebuffer/renderbuffer object
counts are capped at 256 each. Additional shader and draw-work budgets apply.
See the [backend implementation record](../crates/webgl/README.md) for precise limits.
Its opt-in GLSL ES 3.00 language ceiling remains separate from selecting an
actual WebGL2 context.

## Validation and local demo

[The original demo](../tests/pages/webgl.html) draws an interpolated triangle
and a nearest-sampled RGBA texture. The
[compositing demo](../tests/pages/webgl-compositing.html) combines bilinear
filtering and alpha blending, and the [WebGL2 demo](../tests/pages/webgl2.html)
uses GLSL 3.00, VAOs and uint32 indices. The
[framebuffer demo](../tests/pages/webgl-framebuffer.html) samples a rendered
texture while preserving a second canvas. The
[joint pipeline demo](../tests/pages/webgl-pipeline.html) renders two indexed
instances into a texture, then presents it alongside the unchanged default
canvas snapshot. Backend tests assert pixel values for clipping, interpolation,
depth, blending, textures, math built-ins, user helpers, instancing and invalid operations.
Script tests cover API conversion, resource ownership, readback, context exclusivity,
resize and canvas integration; engine tests verify actual page pixels.
These focused checks complement the existing conformance baselines and do
not constitute a WebGL conformance score.
