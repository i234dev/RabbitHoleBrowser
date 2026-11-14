# Collapsible spaces in min-content inline sizing

Isolated candidate from `21d9d89ef550c474446dc21942c3babfdf5e99fd`.
**SIN VALIDAR: no Rust build or runtime has been performed for this change.**
It changes no Grid or Flex algorithm, corpus file, recorded baseline or version.

`inline_intrinsic` previously ended a min-content run at a collapsible space,
then added that pending space to the following word. Thus the min-content width
of `XXXXX XXXXX` included a leading space on the second line and exceeded the
width of the single word. Max-content must keep that same separator.

The pending separator continues to contribute to the max-content line. A soft
break instead saves the preceding min-content run until its following content
is reached, then omits the separator from the new run. A nonwrapping separator
continues to belong to the unbroken run. Word, Atomic/Float and forced Break
consume this pending state; Atomic/Float sizing itself is unchanged.

Opening and closing inline edges need separate causal controls. The previous
Word path repeated the break already taken at Space, which could drop an
opening edge between them. A closing edge of an inline opened before Space
must still finish the preceding run. A stored inline depth distinguishes it
from an inline opened after Space, without an extra traversal or allocation.
Keeping the preceding run pending also avoids fixing its maximum before a
negative closing margin is applied. The references use max-content blocks and
explicit BRs, independently of this min-content state machine.

The ten written layout tests cover collapsed and repeated separators,
max-content, nowrap and ownership of spaces across inline boundaries, NBSP,
pre/pre-wrap/break-spaces preservation, forced and zero-width breaks, literal
hyphen breaks, asymmetric opening/closing MBP, negative margins and consumption
before an atomic. The separate visual pair uses block layout and max-content
with explicit BRs; it does not use Grid or Flex.

This is not a new whitespace or hyphenation implementation. Preserved spaces
and NBSP-containing words retain their existing Piece::Word representation.
Trailing preserved-space hanging, discretionary soft-hyphen insertion
(`text.rs` currently drops U+00AD), dictionary hyphenation and general atomic/
float min-content behavior remain outside this change. The preserved-space
controls deliberately use a longer last word so their expected width does not
depend on that separate hanging-space debt. No expected corpus result is
changed to hide those limitations.

Required validation remains CI on the supported platforms, the own visual pair
before/after, and broader inline/intrinsic/Flex/Grid gates before integration.

Sources: [CSS Sizing3 intrinsic sizes](https://drafts.csswg.org/css-sizing-3/#intrinsic-sizes)
and [CSS Text3 line-edge whitespace processing](https://drafts.csswg.org/css-text-3/#white-space-phase-2).
