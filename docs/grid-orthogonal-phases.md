# Orthogonal Grid phases: interface checkpoint

This successor is **SIN VALIDAR**. The new `grid::phases` module is included
in compilation and unit controls, but has not been built or executed and no
layout caller uses it yet. It does not fix the real
`column-subgrid-orthogonal-writing-mode-002` regression and is not a release
candidate. The existing sizing traversal and all WPT expectations are unchanged.

The pure extraction of `resolve_intrinsic_tracks` and `complete_track_sizing`
still runs both functions immediately. Their boundary is not a legal pause
between parent-axis passes: bases before maximization are not used track sizes.

## Ownership and producer contract

Each intrinsic-min, intrinsic-max or final-layout sizing operation owns a
`GridOperation`. Its identities are local `Rc` tokens, with no global counter,
pointer-key cache or state in `LayoutBox`. Concrete projected item slots include
their row/column spans, line-name provenance and hypothetical lanes position.
Different hypothetical positions therefore cannot retrieve each other's result.

| Parent measurement point | Requested parent axis | Result producer |
| --- | --- | --- |
| Columns | Columns | Complete standalone column sizing or definite inherited columns |
| Rows | Rows | Complete standalone row sizing or definite inherited rows |
| AdjustColumns | Columns | Fresh complete sizing after the opposite-axis pass |
| AdjustRows | Rows | Fresh complete sizing after the opposite-axis adjustment |

An orthogonal wrapper keeps that parent point and maps only the local axis,
spaces, gaps and inherited sources. A clockwise wrapper reverses inner rows;
a counter-clockwise wrapper reverses inner columns; diagonal reflection swaps
without reversing. Names, edge layers and distributed geometry follow the
existing scoped source transform. Two inverse wrappers restore the original
axes. No wrapper restarts a generic child column-first sizing operation.

`CompletedAxis` accepts only complete standalone sizing with an explicit
Definite/MinContent/MaxContent constraint or an inherited axis marked definite.
It rejects placement-only inherited geometry. Recording also retains the
producer point, exact inputs and the completion constraint, so a numeric base
cannot silently be promoted to a completed result.

An unresolved inherited axis can instead record a projection of hoisted
descendants. That projection contains contributions and probes, but no
completed track geometry. It cannot be consumed as a numeric used axis.

Every entry gets a new measurement identity and supplies fresh spaces, gaps
and opposite-axis sources. An old result cannot be retrieved under a newer
identity even if track count and placement are unchanged. Ordinary scoped
intrinsic-cache invalidation remains necessary; it does not invalidate this
caller-owned history automatically. The behavior successor must measure fresh
contributions for each entry and use retained results only with their explicit
producer/input provenance.

## Estimates and adjustment

`PerpendicularEstimate` retains line metadata while exposing no used numeric
geometry if any track in the span has an indefinite maximum. A span with
definite maxima uses those maxima floored by definite minima; content
distribution follows the existing definite-container/all-definite-tracks rule.
An actual definite zero remains geometry, while an infinite estimate remains
tagged. Neither is an instruction to choose a min-content constraint.

Adjustment compares separately supplied **true min-content** probes, including
their descendant path. `Contribution.min` is a minimum contribution and may be
zero after `min-width:0` or overflow rules; it is not that probe. A change only
in maximum contribution or extent does not trigger an adjustment. Once a true
min-content change triggers a pass, both fresh min/max contributions must be
used. Each axis can adjust at most once and the parent points cannot run
backwards.

Retained tracks, names, descendant paths, contribution copies and slots reserve
work before allocating, using the existing per-layout budget. Failed recording
installs no partial result. The existing recursion and scoped restoration
guards remain in force. This is a charged-work cap, not a measured runtime or
complete bound for all preexisting Grid/template work.

## Written controls and remaining gates

Twelve new unit controls cover tagged availability, definite-zero/infinite
estimates and names, maximum/minimum flooring and distributed positions,
clockwise/counter-clockwise/reflected transport, complete maximization versus
intrinsic minimum, fresh geometry with unchanged placement, min-only versus
max-only adjustment triggers, operation/virtual-slot isolation, precharged
recording failure, bounded pass ordering and projection without completed
geometry, including source retention for an empty projection. The split's two existing controls
are retained; their missing test imports are corrected in this checkpoint.
No Rust build or test execution has been performed for this checkpoint.

Next, the behavior successor must honor parent-axis order, resolve the inherited
axis before completing the perpendicular standalone axis with the actual
available space, and perform only the specified bounded adjustments. It must
pass the unchanged 26-case gate, the five own visual pairs, fresh orthogonal
orientation/size-change controls and the agreed wider Grid/Flex/intrinsic gates.
The four old auto-fit failures remain separately classified. In particular,
no stage-order argument substitutes for a real unchanged orthogonal002 PASS.

Primary references: [Grid2 sizing overview §12.1](https://drafts.csswg.org/css-grid-2/#algo-overview),
[Grid2 sizing terms §12.2](https://drafts.csswg.org/css-grid-2/#algo-terms),
[Grid3 containing block](https://drafts.csswg.org/css-grid-3/#containing-block)
and [Writing Modes orthogonal flows](https://drafts.csswg.org/css-writing-modes-3/#orthogonal-flows).

## Finite estimated item measurement: preparatory consumer

The next isolated checkpoint adds helpers in `grid::percentages` and
`grid::estimate`. They are still **not connected to the layout traversal**;
neither the parent-axis order nor orthogonal002 behavior has changed. This
checkpoint itself has not been compiled or executed. The separate interface
snapshot remains unchanged.

The ordinary-item row estimator performs finite layout under the original
tagged available space, without completing perpendicular Grid tracks. Auto
in infinite space realizes a finite max-content width for execution, while
explicit min-content, max-content, fit-content and numeric preferred sizes
keep their own rules. Fit-content under a min-content constraint uses its
minimum; definite available space keeps fit and stretch distinct. Numeric
border-box bounds subtract padding/border once, and minimum wins a min/max
conflict. The numeric proxy never becomes a definite percentage base.

Before executing ordinary layout, a temporary style applies the percentage
rules per property and axis. Non-replaced cyclic preferred/max sizes use their
initial value; cyclic minimum/edge/gap expressions retain their absolute term
with a zero percentage term. The replaced-element min-content distinction is
preserved for the contributing axis. Definite bases, including definite zero,
remain definite. This follows [CSS Sizing3 §5.2.1](https://drafts.csswg.org/css-sizing-3/#cyclic-percentage-contribution)
and the [fit-content definition](https://drafts.csswg.org/css-sizing-3/#fit-content-size).

Margin/padding bases are supplied separately from a box's local inline and
block spaces. Their original containing-block inline reference crosses an
orthogonal wrapper without being swapped. Before passing a numeric proxy to
the ordinary block layouter, edge percentages with a real base are frozen
against that original base. Padding is nonnegative; negative margins and
auto margins retain their separate behavior. The scoped style walk swaps
local axes/spaces along wrapper chains and restores all original `Rc` styles
on unwind, invalidating intrinsic/layout caches before and after measurement.
It precharges each scoped style layer and the existing subtree invalidation
walk. These charges do not claim to measure allocation bytes of ComputedStyle.

The item helper rejects active subgrid requests: those must remain inside the
future per-axis traversal, rather than entering generic whole-box layout.
Ordinary independent descendants can use their existing layout code after
their own box has realized its content width. The helper retains hypothetical
inherited margin layers separately from normalized own percentage edges.

The state now isolates a child's intrinsic-min and intrinsic-max histories in
the same projected slot. An opposite-axis history can be inspected only with
the active operation identity, and retains its old producer/input provenance;
it is not a fresh result of the current entry. Same-writing-mode descendants
preserve the root point's local mapping and supply fresh spaces and edge bases.

Twenty-two additional written controls cover those state extensions, percentage
policies, finite realization, scope restoration/precharge and two ordinary-item
layout comparisons against separate Block references. They make thirty-four
controls in the three new modules, plus the pure split's two controls. They
have not been executed. The parent-axis traversal, changed-min probes from
real projected descendants, mixed fixed/infinite span transport and full
orthogonal orientation/relayout visual controls remain required before
activation, followed by the unchanged 26-case and wider gates. No reference or
baseline is changed to make this preparatory checkpoint pass.
