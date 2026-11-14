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
Every wrapper chain must also have zero padding and border at each level;
ordinary unwrapped siblings can retain their own padding/border. This guard
uses literal computed zero lengths conservatively, so even a zero-valued
percentage/math expression can keep the whole grid on the previous path.
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

`item_measure::measure_ordinary_item` returns a contribution, an independent
min-content value and an optional row baseline. Columns are freshly measured
under both MinContent and MaxContent inline constraints, preserving the
opposite geometry and independent percentage bases. Rows execute at the
caller's current perpendicular inline space. The traversal applies baseline
group shifts and creates probes with its actual item/virtual-slot paths;
the adapter does not invent an operation, complete tracks or reuse a previous
item result. Its eligibility helper checks the same replaced/ratio/subgrid
boundary as the intended first caller.

Eight adapter controls cover distinct minimum/min-content values, intrinsic
bounds independent of numeric preferred widths, repeated geometry changes on
the same ordinary item and each wrapper orientation, defined percentage bases,
unsupported items (including hidden wrapper padding/border) and failure before
any style/result installation. The module
hook belongs to the traversal owner and is deliberately absent from this
adapter-only checkpoint. These tests are not yet compiled or run.

The wrapper PB restriction preserves a concrete unresolved metric boundary:
`wm::wrap` keeps padding/border on its inner logical node while its anonymous
outer style has zero PB, and the wrapper's Fixed width is a physical outer
size. A content-box preferred width of 50px with 10px padding on each side must
be 70px outside, but treating that anonymous style as a PB-free content box
would send 50px to the wrapper. This checkpoint does not repair global WM
geometry or admit that case into the new consumer.
