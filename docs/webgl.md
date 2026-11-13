# Experimental software WebGL1

RabbitHole exposes `canvas.getContext('webgl')` and `experimental-webgl` using
its own Rust shader evaluator, homogeneous clipper and triangle rasterizer in
`crates/webgl`. JavaScript commands produce actual drawing-buffer pixels,
present through canvas and support `readPixels`. No GPU driver, external
shader compiler or embedded browser engine is involved.

This is an initial rendering subset, not a conformant WebGL implementation.
`getContext('webgl2')` returns `null`, extensions are unavailable and modern
Three.js compatibility has not been established. Canvas 2D and WebGL contexts
remain mutually exclusive on a canvas.

## Supported surface

- Shader compilation/linking with status and diagnostic logs, program
  reflection, attributes and float/vector/matrix/sampler uniforms.
- Vertex/index buffers, checked attribute pointers, normalized integer
  attributes, buffer updates and resource deletion.
- Triangles, triangle strips and fans, perspective-correct interpolation,
  clipping, depth tests, viewport, scissor and color/depth write masks.
- RGBA8 2D texture uploads and sub-updates, eight texture units and nearest
  sampling with clamp/repeat/mirrored-repeat. DOM and typed-array uploads
  handle supported unpack flags and row alignment.
- Visible canvas presentation, readback orientation and packing,
  resize behavior, context attributes and loss on oversized drawing buffers.

The JavaScript context accepts a GLSL ES 1.00 subset: straight-line assignments,
initialized local declarations, float/vector/matrix expressions and fragment
`texture2D`. Fragment shaders need an explicit float precision declaration.
Unsupported constructs fail compilation with a diagnostic. Unsupported API
operations report errors rather than pretend to render.

**Set both texture MIN_FILTER and MAG_FILTER to NEAREST.** Texture defaults
retain the standard mipmapped/linear values, but those filters are unsupported
and drawing a defined texture with them fails. Non-power-of-two textures need
clamp-to-edge wrapping for completeness.

## Current limits

No framebuffer objects, stencil, blending, culling, points/lines, multisampling,
cube maps, mipmaps, linear filtering or compressed textures. General GLSL
functions, control flow, arrays, integer/bool variables, derivatives and
discard are unavailable. Context creation does not imply that arbitrary
WebGL applications can run.

Drawing-buffer dimensions are at most 4096 per axis and 4,194,304 pixels in
total. The backend caps vertex attributes at 16, texture units at eight,
buffer storage at 64 MiB and texture storage at a separate 64 MiB, with
additional shader and draw-work budgets. See the
[backend implementation record](../crates/webgl/README.md) for precise limits
and the separate opt-in GLSL ES 3.00 backend mode, which does not enable WebGL2.

## Validation and local demo

[The self-contained demo](../tests/pages/webgl.html) draws an interpolated
triangle and a nearest-sampled RGBA texture. Backend tests assert pixel values
for clipping, interpolation, depth, textures and invalid operations. Script
tests cover API conversion, resource ownership, readback, context exclusivity,
resize and canvas integration; engine tests verify actual page pixels.
These focused checks complement the existing conformance baselines and do
not constitute a WebGL conformance score.
