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

Not implemented: textures/samplers, GLSL functions or control flow, arrays,
integer/bool variables, derivatives, discard, swizzle assignments, framebuffer
objects, stencil, blending, points/lines, culling, multisampling, extensions or
WebGL 2. Textures and shader sampling are rejected, never synthesized.

## Limits and verification

Maximum dimension 4096; at most 4,194,304 framebuffer/readback pixels, 16 vertex
attributes, 8 varying vectors, 32 uniforms, 64 MiB total buffer storage, 65,536
vertices per draw. Shader sources are limited to 64 KiB with bounded expressions
and statements. A draw has a bounded raster work budget and rejects excessive
work before modifying pixels. Limits are explicit to keep software rendering
bounded; they are not a claim of WebGL conformance.

`cargo test -p rabbithole-webgl` checks pixel values for colored and indexed
triangles, perspective interpolation, clipping, depth, normalized attributes,
scissor/masks, resize/readback orientation, shader/link errors and invalid buffer
ranges/resource lifetimes. Browser bindings have additional integration tests.
