# Grid axis consumer: isolated work

This branch starts from the statically reviewed, unhooked `04c1567` checkpoint.
The first checkpoint `e68bea6` contains a real Engine reftest and no layout
change. The later consumer connects the dispatcher to real Grid intrinsic
measurement, track sizing and final item layout. No Cargo, SSH, CI or local
browser execution has been performed for this branch. This is not a stable
delivery or a measured compatibility result.

The test/reference HTML files are byte-identical copies of the previously
frozen `ec9cee0` pair `wrapper-definite-perpendicular`. Five adjacent physical
10x20px inline blocks, with zero font/line height, yield five columns in a
30px perpendicular span and two columns in a 60px span. The reference uses
ordinary Block boxes of physical widths 50px/20px, not Grid or Flex sizing.
Clockwise, reflected and counter-clockwise wrappers exercise the 30px case.

The original pair's official 21d and 3c baseline executions on 2026-11-14 both
failed with 2800 differing pixels. The fixture-only run used JIT off, normal
GC, one worker, nice19/CPU2, an 800000KiB address-space limit and a 20s process
deadline. Its receipt and PNGs remain in `grid-estimate-review-2026-11-14/v2`;
`runtime/receipt.json` has SHA256
`efd37ea4378e3b4a8758b92e20b79b90813fefb41d6c0650c5d6547347b4d68c`.
That historical result is not a new
execution of `04c1567` or of this test checkpoint. The external WPT corpus,
orthogonal002 and mix2 oracles are untouched.

The consumer will own parent-pass ordering, scoped child operations, projection
and final track completion. The ordinary-item adapter owns finite measurement
under tagged available spaces and real containing-block bases. It must never
complete unresolved perpendicular tracks or convert a numeric execution proxy
into a definite percentage reference. Actual subgrid recursion is intercepted
before ordinary whole-box layout, and each concrete projected slot retains
separate intrinsic-min, intrinsic-max and final-layout histories.

No wiring is authorized into stable. A completed isolated consumer must first
pass this Engine case, the unchanged orthogonal002/mix2 gates and retained
cases, then the agreed broader Grid/Flex/intrinsic checks. Root owns compilation
and execution after the current MCP/Koma fixes; this branch does not launch
them.

## First real consumer boundary

The first hook is restricted to independent ordinary Grid containers with an
in-flow `wm_wrapper` item. The whole container uses the previous path if any
in-flow sibling requests an inherited axis or is replaced/ratio-sized, including
an image or ratio hidden behind wrappers. Wrapper chains with nonzero or
unresolved padding/border also stay on the previous path: their physical
content-box metrics need a separate measured fix. Ordinary unwrapped siblings
with padding/border remain supported. Empty or oversized wrapper chains are
rejected by the same adapter eligibility guard. Margins are distinct from
padding/border. Grids without an ordinary orthogonal item use the previous
path; there is no selector based on a URL, fixture name or expected result.

Each intrinsic minimum, intrinsic maximum and final layout creates its own
caller-owned operation. Columns, rows and their two bounded adjustment points
feed fresh item measurements into `grid_axis_pass`; the caller installs only
the requested CompletedAxis. Per-item constraints and actual percentage bases
come from that item's span, using compatible used geometry or a tagged fixed
maximum estimate. A finite proxy is never fed back as a percentage base.
Baseline shims belong to both the row contribution and its adjustment probe.

An own-axis used span can supply numeric measurement geometry while its size
still depends on the item whose contribution is being computed. The caller
therefore keeps that contributing-axis percentage base indefinite unless the
span is independently fixed. The current conservative proof is equal fixed
minimum/maximum track sizes (or collapsed tracks), not a complete classifier
for every minmax/flex case. This applies to auto rows and to auto columns in a
definite non-stretching container. The already-requested opposite-axis span
continues to supply geometry/bases for cross-axis measurement. Final item
sizing can resolve percentages against the actual used area without sizing
the container again, following
[CSS Sizing3 §5.2.1](https://www.w3.org/TR/css-sizing-3/#cyclic-percentage-contribution).

The additional `cyclic-inline-percentage` Engine pair uses min-/max-content
grids with centered auto columns and a definite 80px payload, plus an auto-row
percentage-height case. Its ordinary Block reference has an 80px container
and 40px percentage-sized child; the row reference reserves the 20px intrinsic
row while its child resolves to 10px. This is a regression control for the new
consumer, with no before/after execution claimed. The original wrapper pair
remains byte-identical.

Finalization also uses the area's real constraints/bases to select the item
width, instead of reentering the viewport-based wrapper intrinsic helper.
Actual final execution freezes dimensional and edge percentages before WM
transposes its numeric containing block. It preserves the resulting geometry
and restores original styles/caches through the scoped guard. The shared
legacy finalizer receives the resolved width; its remaining body is unchanged.

The adapter keeps `Contribution.min` as the previous minimum contribution and
supplies true min-content separately. The track solver still conflates these
for some Auto/MinContent functions; this checkpoint does not repair that debt.
The adapter's plain preferred-bound/percentage Engine pairs remain preserved
in the separate TEST-ONLY checkpoint `e7f66ba`; this narrower consumer does not
claim to exercise or recover them. Neither subgrid recursion, lanes stacking
nor orthogonal002/mix2 is transferred to the new path by this hook.

## Shared recursion guard follow-up

Static peer review of `ddfa29e` found that the new ordinary consumer entered
the charged-work budget but not its shared recursion counter. An item estimate
can reenter an independent ordinary Grid through intrinsic/layout measurement,
so this new route also needs the existing 128-entry bound. This was a static
missing-guard finding, not an observed crash or a complete audit of all legacy
layout recursion.

The successor keeps a `DepthGuard` for the entire ordinary sizing operation.
It delegates to the same budget entry used by subgrid projection, so mixed
projection/ordinary recursion shares the limit and RAII restores it on normal
return or unwind. A real-consumer control saturates the counter, verifies
rejection before style/cache mutation, drops the outer guards and retries the
consumer, then checks that its normal return released the slot. A second
control interleaves projection with ordinary sizing and checks unwind restores
the projection's depth. These two controls, the earlier fourteen new unit
controls and both Engine tests remain uncompiled and unexecuted here.
