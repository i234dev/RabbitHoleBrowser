# Uniform sequence validation

This successor is based on final 0.1.10
`4f8717910d698949fd3e39477e67bef633da900b` and replays the isolated uniform
changes from `e901c61324e0095c9ff9da30bebdf0e688ef20e7` with two review fixes.
Tests described here are written but have not been executed for this candidate.

## Rules checked against primary sources

[WebGL 1 §5.14.10](https://registry.khronos.org/webgl/specs/latest/1.0/#5.14.10)
and [WebGL 2 §3.7.8](https://registry.khronos.org/webgl/specs/latest/2.0/#3.7.8)
add array-length validation to the GLES uniform API. An empty direct sequence
is too short, including for `uniform1fv` and `uniform1iv`.

| Input after IDL conversion | Result |
| --- | --- |
| Non-null valid location; length below method width or not divisible by it | `INVALID_VALUE` |
| Non-array uniform; complete data for more than one element | `INVALID_OPERATION` |
| Complete single element, incompatible setter/uniform type | `INVALID_OPERATION` |
| Matrix transpose true, otherwise valid non-null call | `INVALID_VALUE` |
| Null location, convertible input | No update and no GL error, including empty data or transpose true |

Method width is 1–4 for vector setters and 4/9/16 for square matrix setters.
This is not an explicit GLES `count=0` call: the WebGL array-length rule applies.
The [GLES uniform reference](https://registry.khronos.org/OpenGL-Refpages/es2.0/xhtml/glUniform.xml)
has a separate signed count argument and rejects negative counts; multiple
elements targeting a non-array uniform are invalid.

[Web IDL nullable conversion](https://webidl.spec.whatwg.org/#es-nullable-type)
maps both null and undefined to null. Its
[argument conversion algorithm](https://webidl.spec.whatwg.org/#es-overloads)
converts the location brand before data and completes argument conversion before
invoking the GL operation. Consequently an unbranded location throws before data
iteration, while a branded stale/foreign location or null does not skip numeric
conversion. A conversion exception does not itself set a GL error.
[Sequence conversion](https://webidl.spec.whatwg.org/#es-sequence)
reads the iterator method once, consumes every element, and does not close the
iterator when an element's numeric conversion throws. Callable objects can be
sequences. These tests do not prescribe priority between simultaneous GL errors.

## Implementation and limits

Float vectors, scalar bool/sampler integer setters, and square matrices share a
converter. It counts the complete sequence while retaining at most 16 numeric
elements. The normal script watchdog bounds execution; no new early truncation
or array-length cap changes sequence semantics. Typed arrays use their captured
intrinsic length getter rather than an overridable page property. Conversion
and validation finish before a native command can write a uniform; native copies
receive at most one supported element. Scalar wrappers do not iterate arrays.
The native bridge is invoked with captured `Reflect.apply` and a null-prototype
array-like argument list; the retained sequence is copied using its already computed size.
Both operations avoid consulting a public iterator or typed-array length getter.

GLSL uniform arrays, integer vector types, and WebGL 2 source-offset/source-length
overloads remain unsupported. The `uniform2iv`–`uniform4iv` wrappers perform
brand/data/null/length handling, then reject non-null complete calls explicitly;
they do not create integer-vector support. Raw backend `uniform_f32` remains an
exact-value internal helper; the component-aware WebGL command validates lengths.

## Causal coverage and external provenance

The Khronos WebGL CTS pin is
`39443210f7f81db4773eb104facadfb85d05dfa4`, inspected as source only:

- [uniformfBadArgs](https://github.com/KhronosGroup/WebGL/blob/39443210f7f81db4773eb104facadfb85d05dfa4/sdk/tests/conformance/more/functions/uniformfBadArgs.html)
  distinguishes empty/partial/non-multiple data from a complete non-array count.
- [uniformiBadArgs](https://github.com/KhronosGroup/WebGL/blob/39443210f7f81db4773eb104facadfb85d05dfa4/sdk/tests/conformance/more/functions/uniformiBadArgs.html)
  explicitly expects `INVALID_VALUE` for `uniform1iv(location, [])`.
- [uniformMatrixBadArgs](https://github.com/KhronosGroup/WebGL/blob/39443210f7f81db4773eb104facadfb85d05dfa4/sdk/tests/conformance/more/functions/uniformMatrixBadArgs.html)
  covers matrix lengths, dimensions and transpose.
- [null-uniform-location](https://github.com/KhronosGroup/WebGL/blob/39443210f7f81db4773eb104facadfb85d05dfa4/sdk/tests/conformance/uniforms/null-uniform-location.html)
  covers null no-ops across uniform families.

Three new backend tests and the Script/Engine fixture exercise both context
versions. Every supported family checks initial pixels, rejected writes and
readback. Other controls cover conversion after element 17, exact long lengths,
iterator caching, null/undefined, Symbol/BigInt, location brands, relink/foreign
locations, and scalar argument order. Causal controls poison `Array.iterator`
and `%TypedArray%.prototype.length`, restore both in `finally`, and require
different native readback and pixel values. An explicit throwing-spread control
prevents a broken VM optimization from hiding the native-call regression.
A throwing numeric setter on `Object.prototype` must also leave the native
argument list, uniform readback and changed pixels unaffected.
Run Script tests with GC stress both off
and on. No CTS suite or full WebGL conformance result is claimed.
