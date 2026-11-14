# Synthetic font style controls

`font-synthesis-style: none` suppresses a synthetic slant when an italic or
oblique face is missing. `auto` permits the existing synthesis. The longhand
inherits and supports `inherit`, `initial` and `unset`; it shares the style
component with the existing `font-synthesis` shorthand, so declaration order
determines that component. Invalid longhand values preserve earlier valid
declarations.

The control affects synthesis, not the selection of a real italic face. The
pixel regression uses the bundled serif family, which has no italic face, and
compares alternating upright/slanted lines against a reference that does not
use the longhand. A second, all-upright reference must differ, so disabling all
synthesis cannot produce a passing test.

`oblique-only` is not implemented: the current style/face model does not retain
the distinction required between italic and oblique fallback. Its declarations
have no effect. Font-synthesis weight, small-caps and position, variable-font
axes and complete Fonts Level 4 conformance are outside this change.

See [CSS Fonts Level 4, synthesis style](https://www.w3.org/TR/css-fonts-4/#font-synthesis-style).
