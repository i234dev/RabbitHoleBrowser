# Experimental software WebGL backend

This is an initial rendering milestone, not a conformant WebGL implementation.
It uses RabbitHole's own Rust shader evaluator, homogeneous clipper and
triangle rasterizer. There is no GPU driver, ANGLE, browser engine or external
shader compiler dependency.

The API follows [Khronos WebGL 1](https://registry.khronos.org/webgl/specs/1.0/)
and the linked GLSL ES 1.00 language specification for the implemented subset.
Unsupported shader constructs fail compilation with a diagnostic; unsupported
operations must be rejected by the bindings instead of reporting fake success.

## Implemented

- RGBA8 drawing buffer and floating-point depth buffer, initialized to transparent
  black and depth 1. `resize` clears them and preserves all GL state, including
  viewport and scissor, as required by WebGL's viewport rules.
- Clear color/depth, viewport, scissor, color/depth write masks and all eight depth
  comparisons. Clear obeys the scissor and write masks, independently of viewport.
- Core WebGL1 blending: independent RGB/alpha factors and add/subtract/reverse
  equations, constant blend color, source-alpha saturation and atomic invalid-state
  rejection. Blending uses raw normalized RGBA before color write masks; clear
  bypasses blending. RGB-only canvas buffers use destination alpha one.
- Vertex/index buffers with zero-initialized storage, sub-data updates, checked
  captured attribute pointers, and actual deletion. Attributes accept float,
  signed/unsigned bytes and shorts, including normalized conversions; disabled
  arrays use constant attribute values with `(0, 0, 0, 1)` defaults.
- Draw arrays/elements for triangles, triangle strips and triangle fans. Index
  formats are unsigned byte/short and, in WebGL2, unsigned int. Ranges are
  checked before drawing. Homogeneous clipping covers all six clip planes, varying interpolation
  is perspective-correct, depth is interpolated in window space, and a consistent
  half-open edge rule prevents shared-edge holes or double coverage.
- Shader objects, compile status/logs, program linking, active attribute locations,
  uniforms and deferred deletion of attached shaders/current programs.
- Raw RGBA `read_pixels` in lower-left GL order; out-of-buffer pixels are zero.
  `framebuffer_rgba` exposes top-left rows for presentation. Color channels are
  raw shader outputs; canvas context attributes determine how the compositor
  interprets alpha. Blending uses raw RGBA; no implicit canvas premultiplication is performed here.

## GLSL subset

One `void main()` with straight-line, whole-variable assignments and initialized
local declarations. Global `attribute` (vertex only), `varying`, and `uniform`
declarations; scalar `float`, `vec2/3/4`, and uniform/local `mat4` types. Precision
qualifiers, `precision ... float;`, comments and optional `#version 100` are
recognized. Fragment shaders require an explicit float precision declaration.
Supported expressions are literals, variables, vector swizzles,
constructors, unary signs and `+ - * /`; matrix/vector and matrix/matrix
multiplication use GLSL column-major semantics. Decimal integer literals within
the exact f32 range may be used in constructors; integer arithmetic and implicit
integer-to-float assignment are rejected rather than evaluated as float math. Matrix uniform updates require
`transpose=false`.

Vertex shaders must write `gl_Position`; fragment shaders must write
`gl_FragColor` and can read `gl_FragCoord`. Active cross-stage varying types and
shared uniform types must agree. Uniforms start at zero after linking.

The evaluator also supports 32 typed float/vector math built-ins, with checked
argument counts, types and expression costs. See
[the supported functions](../../docs/webgl.md#shader-language-subset).
User helpers may return `float` or `vec2/3/4`, take up to eight named parameters
of those types by value (ordinary or `in` parameters), and use initialized locals
plus one final `return` expression. Precision qualifiers and zero-argument helpers
are accepted. Definitions must precede their callers and `main`; helpers may call
earlier helpers, the math built-ins and supported fragment texture operations.
They can read immutable globals such as uniforms, vertex attributes and fragment
inputs. Parameters and locals may shadow globals; nested calls read the original
global environment rather than capturing a caller's local values. Helpers cannot
write globals or read mutable outputs. These calls follow the supported portion
of [GLSL ES function definitions and input parameters](https://registry.khronos.org/OpenGL/specs/es/2.0/GLSL_ES_Specification_1.00.pdf), section 6.1.

At most 16 helpers and 16 nested function calls are supported. Function bodies
share immutable ASTs; the 512-unit expression budget includes every nested call
site, argument, body and local binding so shared bodies do not hide exponential
execution. Unused helpers do not activate their referenced uniforms. Recursion,
forward calls, prototypes, overloading/redefinition, `out`/`inout`/`const`
parameters, extra statements after the final return and general control flow are
rejected with diagnostics; they are not silently ignored.

General control flow, arrays, integer/bool variables, derivatives,
discard, swizzle assignments, stencil, points/lines, culling, multisampling and
extensions remain unsupported. Texture sampling has the explicit bounded subset
below. Unsupported shader operations fail compilation with diagnostics.

## Limits and verification

Maximum dimension 4096; at most 4,194,304 framebuffer/readback pixels, 16 vertex
attributes, 8 varying vectors, 32 uniforms, 64 MiB total buffer storage, 65,536
vertices per draw. Shader sources are limited to 64 KiB with bounded expressions
and statements. A draw has a bounded raster work budget and rejects excessive
work before modifying pixels. Limits are explicit to keep software rendering
bounded; they are not a claim of WebGL conformance.

`cargo test -p rabbithole-webgl` checks pixel values for colored and indexed
triangles, perspective interpolation, clipping, depth, normalized attributes,
scissor/masks, blend factors/equations and alpha-channel behavior, resize/readback
orientation, shader/link errors and invalid buffer ranges/resource lifetimes. Browser bindings have additional integration tests.

## Texture milestone

RGBA8 2D texture storage with level-zero uploads/sub-updates, zero initialization,
8 texture units, texture deletion, and actual fragment `sampler2D` /
`texture2D(sampler, vec2)` evaluation. Sampler uniforms use `uniform_i32` and
appear in program reflection as `SAMPLER_2D`. Upload rows start at the lower-left;
the bindings handle DOM conversion and pixel-unpack flags before calling here.
Textures have a separate 64 MiB total storage budget and the framebuffer size
limits apply to each image.

Nearest and bilinear sampling support clamp-to-edge, repeat and mirrored-repeat;
non-power-of-two images require clamp-to-edge in both dimensions under WebGL 1
completeness rules. Missing/uninitialized textures, and NPOT-repeat textures in
WebGL1, sample opaque black for incomplete textures; allocated zero-filled images
sample their actual transparent zero pixels.

**Set both MIN_FILTER and MAG_FILTER to NEAREST, or both to LINEAR.** Bilinear
filtering uses texel-center coordinates and the selected wrap mode. Defaults
retain GL's mipmapped-min/linear-mag values, but mipmaps and mixed min/mag filters
are unsupported. A defined image with those unsupported filters reports
INVALID_OPERATION before pixels change. Derivative-based LOD, mipmap generation,
cube maps, compressed data, vertex texture sampling and texture extensions remain
unsupported. `tests/linear.rs` checks interpolated pixels and rejected states.

`tests/textures.rs` verifies texel values/orientation, sub-updates, texture units,
reflection, wrapping, NPOT behavior, resource bounds, explicit unsupported-state
errors and malformed sampler shaders.

## Explicit GLSL ES 3.00 backend mode

`Context::new` continues to permit only GLSL ES 1.00. The separate backend
constructor `Context::new_with_glsl_version(width, height, max_version)` accepts
100 or 300 as a **maximum source-language version**, not as a WebGL context
version. Mode 300 also accepts version-100/default sources using version-100
rules, as described by [Khronos GLSL ES 3.00 support](https://registry.khronos.org/webgl/specs/2.0/#GLSL_ES_3.00_support).
A program may not link vertex and fragment stages with different versions.
Selecting this backend language mode alone does not change the JavaScript
context version; the separate experimental WebGL2 milestone is described below.

The [GLSL ES 3.00](https://registry.khronos.org/OpenGL/specs/es/3.0/GLSL_ES_Specification_3.00.pdf)
subset requires `#version 300 es`. Vertex `in` maps to attributes and vertex
`out` to varyings; fragment `in` consumes varyings and exactly one declared
`out vec4` supplies the existing RGBA drawing buffer. Fragment `texture()` uses
the actual sampler2D implementation; legacy `attribute`, `varying`,
`texture2D()` and `gl_FragColor` are rejected in version-300 sources.

Literal `layout(location=N)` is supported for vertex inputs at 0..15 and the
single fragment output at location 0. Explicit source locations override
`bind_attrib_location`; aliasing active inputs fails linking. Other layouts,
multiple fragment outputs/MRT, non-vec4 outputs and newer language versions fail
explicitly. The existing arithmetic, texture, no-control-flow and resource
limits remain in effect. Precision declarations for float, int and sampler2D
are recognized; integer variables/arithmetic remain unsupported.

Token limits are 256 characters in the default backend mode and 1024 in mode
300, including version-100 sources there. Reserved `webgl_`/`_webgl_` identifiers
are rejected. `tests/glsl300.rs` verifies colored/textured triangles, named
outputs, location precedence/alias errors, mixed-version links, source-version
restrictions, token limits, and legacy-100 rendering under the optional mode.


## Experimental WebGL2 context milestone

`Context::new_webgl2` selects a distinct context version and admits the GLSL ES
3.00 subset above. `new_with_glsl_version(..., 300)` alone remains a WebGL1
backend context: a source-language ceiling is not a context version.

The WebGL2 milestone adds vertex array objects with captured attribute pointers,
enables and element-array bindings. ARRAY_BUFFER and generic attribute constants
remain global. Deleting a buffer detaches current bindings while storage remains
alive for inactive VAOs that still reference it; deleting those VAOs releases
otherwise unused storage. UNSIGNED_INT element indices have checked alignment
and bounds. Fixed primitive restart applies to byte, short and uint indices in
WebGL2, including strips/fans, and never changes WebGL1 index semantics. NPOT
textures can repeat in WebGL2 within the supported texture/filter subset.

JavaScript has a distinct experimental `WebGL2RenderingContext`; it preserves
canvas context exclusivity and rejects unimplemented commands and extra offset
upload/readback overloads. This is not a conformant WebGL2 implementation or a
Three.js compatibility claim. Integer attributes/uniforms, multiple color
attachments/MRT, 3D textures, UBOs, queries, samplers, synchronization and transform
feedback remain unsupported. Existing software limits still apply.

`tests/vertex_arrays.rs` and the script WebGL tests check actual GLSL300 pixels,
VAO switching/resource lifetimes, 32-bit indices, restart and WebGL1 isolation.


## Instancing through vertex attributes

WebGL2 `vertexAttribDivisor`, `drawArraysInstanced` and `drawElementsInstanced`
advance enabled attributes by `floor(instance/divisor)` when the divisor is
nonzero. A zero divisor remains vertex-indexed. Divisors belong to the bound
VAO; generic constant attributes remain context-global. Non-instanced draws use
instance zero for divisor attributes. Indexed draws preserve all supported index
widths and fixed primitive restart.

Every instance's attribute range, aggregate vertex work, fragment work and
prepared geometry budget is checked before the first framebuffer write. Current
software limits are 1024 instances, 262,144 evaluated vertex references, 32 Mi
weighted work units and 64 MiB of prepared triangle data per draw. A rejected
batch leaves existing pixels and depth intact. WebGL1 and its extensions do not
expose instancing.

`gl_InstanceID` is explicitly unsupported until the shader evaluator has real
integer types; it fails shader compilation rather than being simulated as a
float. This milestone uses actual divisor attributes for per-instance positions,
colors and other data. See `tests/instancing.rs` for pixel, divisor, VAO, restart,
range and atomic-budget regressions. The whole batch selects its framebuffer
once, validates every instance, then executes in order; target restoration also
applies to rejected batches.

## Framebuffer milestone

Framebuffer objects render into RGBA8 level-zero 2D textures. Clears, draws,
readback and subsequent texture sampling use real attachment pixels, while
canvas presentation continues to use only the default drawing buffer. Binding
an FBO preserves the viewport, masks and other GL state. WebGL2 read and draw
framebuffer bindings are independent; WebGL1 rejects those separate targets.

Depth renderbuffers support DEPTH_COMPONENT16, with initialized depth one and
16-bit quantization. Depth-test enable state is independent of whether the
default canvas requested depth: an attached FBO depth buffer still works when
that context was created with depth:false. Clears respect masks and scissor.

Missing/unallocated attachments, size mismatches and feedback loops are
validated before pixels change. Sampling a texture attached to the current
draw framebuffer is rejected. Deleted attachments are retained by inactive
FBOs, and deleting an attachment detaches it from current framebuffer bindings.

The current subset requires COLOR_ATTACHMENT0 to hold an RGBA8 2D texture at
level zero. Optional depth storage must have matching dimensions. Depth-only
FBOs, color renderbuffers, stencil, multiple attachments/MRT, multisampling and
blits remain unsupported. Mismatched dimensions are INCOMPLETE_DIMENSIONS in
WebGL1 and explicitly FRAMEBUFFER_UNSUPPORTED in the WebGL2 subset. Renderbuffers
have a separate 64 MiB budget and at most 256 live objects of each FBO kind.
The existing framebuffer dimension and pixel-count limits also apply.


## Combined validation

`tests/instancing.rs` includes a joint FBO regression covering two differently
colored instances, an unchanged default buffer, independent read/draw bindings,
alpha:false/depth:false default attributes, attached depth16 and restoration
after a rejected batch. Script tests exercise the same composition through the
JavaScript API. Engine fixtures check both native canvas pixels and actual page
pixels, including sampling the resulting texture into the visible canvas. See
[the joint pipeline demo](../../tests/pages/webgl-pipeline.html).

These checks cover the implemented subset. Multiple color attachments/MRT,
stencil, blits and multisampling remain open; there is no full WebGL2 conformance
or Three.js compatibility claim.
