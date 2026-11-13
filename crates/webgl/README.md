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
  formats are unsigned byte/short; all array/index ranges are checked before
  drawing. Homogeneous clipping covers all six clip planes, varying interpolation
  is perspective-correct, depth is interpolated in window space, and a consistent
  half-open edge rule prevents shared-edge holes or double coverage.
- Shader objects, compile status/logs, program linking, active attribute locations,
  uniforms and deferred deletion of attached shaders/current programs.
- Raw RGBA `read_pixels` in lower-left GL order; out-of-buffer pixels are zero.
  `framebuffer_rgba` exposes top-left rows for presentation. Color channels are
  raw shader outputs; canvas context attributes determine how the compositor
  interprets alpha. No blending or implicit premultiplication is performed here.

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

Not implemented: general GLSL functions or control flow, arrays,
integer/bool variables, derivatives, discard, swizzle assignments, framebuffer
objects, stencil, points/lines, culling, multisampling, extensions or
WebGL 2. Texture sampling has the explicit bounded subset below. Unsupported
shader operations are rejected, never synthesized.

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
orientation, shader/link errors and invalid buffer
ranges/resource lifetimes. Browser bindings have additional integration tests.

## Texture milestone

RGBA8 2D texture storage with level-zero uploads/sub-updates, zero initialization,
8 texture units, texture deletion, and actual fragment `sampler2D` /
`texture2D(sampler, vec2)` evaluation. Sampler uniforms use `uniform_i32` and
appear in program reflection as `SAMPLER_2D`. Upload rows start at the lower-left;
the bindings handle DOM conversion and pixel-unpack flags before calling here.
Textures have a separate 64 MiB total storage budget and the framebuffer size
limits apply to each image.

Nearest-neighbor sampling supports clamp-to-edge, repeat and mirrored-repeat;
non-power-of-two images require clamp-to-edge in both dimensions under WebGL 1
completeness rules. Missing/uninitialized or NPOT-repeat textures sample opaque
black, as required for incomplete textures; allocated zero-filled images sample
their actual transparent zero pixels.

**Set both MIN_FILTER and MAG_FILTER to NEAREST.** Object defaults retain GL's
real mipmapped-min/linear-mag defaults, but linear and mipmapped filtering are
not implemented: setting those filters reports an error; drawing a defined image
with unsupported default filters reports INVALID_OPERATION without changing
pixels. No derivative-based LOD, mipmap generation, cube maps, compressed data,
vertex texture sampling or texture extensions are claimed.

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
Nothing here enables JavaScript `getContext('webgl2')` or claims WebGL2 support.

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
