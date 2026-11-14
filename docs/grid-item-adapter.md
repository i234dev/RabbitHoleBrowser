# Ordinary item adapter checkpoint

This source is isolated from stable and has not been compiled or run. Its
code commits do not depend on the TEST-ONLY Engine pairs in `e7f66ba`.

`estimate::ItemSpace` separates per-area sizing constraints, actual containing
block bases and the original inline base for percentage edges. The ordinary
estimate receives all three. Normalization uses the actual bases; realization
uses the constraint separately. Definite percentages in preferred/min/max
sizes are frozen to CSS pixel lengths in each local wrapper axis before
ordinary layout executes against a finite proxy. Margins/padding keep their
independent original-inline base. Scopes restore every original style and
invalidate measurement caches on return or unwind.

The old `estimate_row_item` entry still supplies equal constraints/bases for
its earlier helper contract. New controls use the actual entry end to end:
MinContent with a fixed percentage base, auto under Min/MaxContent, invalid
definite dimensions, and percentage dimensions through clockwise,
counter-clockwise and reflected wrappers. They are written tests, not passes.

The first consumer is restricted to ordinary Grid items, without inherited
axis requests, replaced items or aspect ratios hidden by wrapper chains.
The grid traversal must reject the entire new path if any item is unsupported;
it must not silently omit that sibling or mix incomplete measurements. The
IntrinsicMin replaced-item transfer from the opposite minimum remains a
separate gate before broadening this eligibility.

This does not connect a real caller by itself, prove orthogonal002/mix2, fix
all writing-mode geometry, or establish Grid conformance. In particular,
minimum contribution and min-content contribution remain separate result
data: the existing track solver still conflates its single `Contribution.min`
input for some sizing functions. The first adapter keeps that legacy minimum
contract and supplies true min-content separately for adjustment probes.
