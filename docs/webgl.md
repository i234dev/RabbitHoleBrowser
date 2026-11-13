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
- Visible canvas presentation, readback orientation and packing,
  resize behavior, context attributes and loss on oversized drawing buffers.

## Shader language subset

WebGL1 accepts a GLSL ES 1.00 subset: straight-line assignments,
initialized local declarations, float/vector/matrix expressions and fragment
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

These built-ins do not implement user-defined functions, loops, conditionals,
arrays, general integer/bool arithmetic or derivatives. Unsupported overloads
fail shader compilation.

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

This milestone does not include instancing, framebuffer objects/MRT, integer
attributes or general integer uniforms, 3D textures, UBOs, queries, sampler
objects, synchronization or transform feedback. Extra offset upload/readback
overloads are also unsupported. Exposed unsupported operations report errors.

## Texture filtering

Set both `MIN_FILTER` and `MAG_FILTER` to `NEAREST`, or both to `LINEAR`.
`LINEAR` interpolates four texels using texel-center coordinates and the selected
wrap mode. Texture-coordinate derivatives and LOD selection are unfinished;
mixed minification/magnification filters and mipmaps are unsupported and fail
before drawing. Defaults retain the standard mipmapped/linear state, so a new
defined texture needs supported filters explicitly. In WebGL1, non-power-of-two
textures also need clamp-to-edge wrapping for completeness.

## Current limits

No framebuffer objects, stencil, culling, points/lines, multisampling,
cube maps, mipmaps or compressed textures. User-defined GLSL
functions, control flow, arrays, integer/bool variables, derivatives and
discard are unavailable. Context creation does not imply that arbitrary
WebGL applications can run.

Drawing-buffer dimensions are at most 4096 per axis and 4,194,304 pixels in
total. The backend caps vertex attributes at 16, texture units at eight,
buffer storage at 64 MiB and texture storage at a separate 64 MiB, with
additional shader and draw-work budgets. See the
[backend implementation record](../crates/webgl/README.md) for precise limits.
Its opt-in GLSL ES 3.00 language ceiling remains separate from selecting an
actual WebGL2 context.

## Validation and local demo

[The original demo](../tests/pages/webgl.html) draws an interpolated triangle
and a nearest-sampled RGBA texture. The
[compositing demo](../tests/pages/webgl-compositing.html) combines bilinear
filtering and alpha blending, and the [WebGL2 demo](../tests/pages/webgl2.html)
uses GLSL 3.00, VAOs and uint32 indices. Backend tests assert pixel values
for clipping, interpolation, depth, blending, textures, math built-ins and invalid
operations. Script
tests cover API conversion, resource ownership, readback, context exclusivity,
resize and canvas integration; engine tests verify actual page pixels.
These focused checks complement the existing conformance baselines and do
not constitute a WebGL conformance score.
