# Grid intrinsic recovery: diagnostic checkpoint

This branch is **SIN VALIDAR** and **not integrable into a release**. It depends
on the computed subgrid metadata, explicit GridSpace constraints and normal-gap
provenance commits. The published 0.1.9 and frozen 0.1.10 source are unchanged.

The implementation transports immutable parent track sizes, positions, collapsed
state, names and gaps to a requested subgrid axis. Orthogonal wrappers map and
reverse those axes. Scoped installation restores computed styles and invalidates
caches, including when a measurement unwinds. Automatic spans use the number of compact local line-name groups; explicit
spans and distinct start/end lines keep their specified span. Percentage edge references distinguish placement-only intrinsic geometry from
used areas. Scoped percentage margins/padding resolve in the grid area once,
including through orthogonal wrappers; styles are restored afterwards. Shared
axes ignore preferred/min/max sizes and self alignment, cannot
append implicit tracks, and retain ancestor edge/gap layers through nesting.

Descendants contribute to the corresponding parent tracks. An automatically
placed subgrid in a lanes parent projects every sub-item into all possible
parent tracks, ignoring its own explicit local placement for sizing only.
Automatic items inside a fixed lanes subgrid receive the largest possible layer
of margin, border and padding at each edge in every hypothetical placement. A grid performs an
initial column/row pass and at most one adjustment of each axis. The initial
perpendicular estimate uses definite maximum track sizes, floored by definite
minimums. Initial estimates apply content distribution only when the container
and all tracks are definite; adjusted estimates use the actual opposite-axis
available size and alignment. Intrinsic versus definite-zero geometry is
carried explicitly. An empty subgrid does not synthesize padding contributions; a nonempty
subgrid with empty edge tracks creates the hypothetical edge items required by
Grid2. Identical contribution spans are compacted in place using independent
min/max maxima. Row-lanes measurement receives definite inherited geometry
before computing inline contributions; explicit intrinsic preferred sizes remain
in force in an independent axis. Stretch relayout keeps inherited geometry.

Written, not executed: twenty-five geometry/compaction/unwind/budget unit tests, nineteen layout tests,
two new visual reftest pairs and the seven prior GridSpace tests. The original
four deferred intrinsic constraint fixtures are preserved byte-for-byte. Style
metadata/gap tests are in the dependency commits.

Blocking validation and algorithm limits:

- Neither Rust compilation nor runtime tests have been performed by this agent.
- The two previously passing subgrid WPT controls remain mandatory gates. The
  new automatic-inline control targets row grid-lanes-subgrid-005a, but is not
  evidence of a pass. The content-dependent orthogonal case is still pending:
  standalone perpendicular track state is not yet retained between the initial
  intrinsic estimate and final orthogonal layout. The bounded adjustment alone
  must not be presented as a complete solution.
- New subgrid work has a per-Layouter budget of 1,000,000 charged transport,
  placement and contribution units (including full track/name copies and subtree
  invalidation visits, reserved before that work) and a maximum of 128 active projection frames.
  Exhaustion explicitly fails layout through the existing panic boundary; it
  does not return a truncated contribution set. Scope guards restore styles and
  recursion counters while unwinding. Ordinary Grid work is not charged. This
  counter does not claim to fix the existing unbounded parser/template expansion
  or other layout algorithms. The error contract and limits require root review.
- This is not complete subgrid conformance: cross-subgrid baseline alignment and
  clipping/recreation of inherited implicit named-area edge names require further
  work and dedicated controls.

Release gates: restore all eleven deferred intrinsic gains, retain the thirteen
improvements shipped in 0.1.9, and preserve row grid-lanes-subgrid-005a plus
column-subgrid-orthogonal-writing-mode-002. Four other auto-fit failures already
present in the stable control must not be hidden or assigned new expectations.
No baseline or WPT corpus file is changed.

Algorithm references (drafts; no full conformance claim):

- <https://drafts.csswg.org/css-grid-2/#algo-overview>
- <https://drafts.csswg.org/css-grid-2/#subgrids>
- <https://drafts.csswg.org/css-grid-3/#subgrids>
- <https://drafts.csswg.org/css-grid-3/#intrinsic-sizes>
- <https://drafts.csswg.org/css-writing-modes-3/#orthogonal-flows>
