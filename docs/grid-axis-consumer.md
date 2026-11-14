# Grid axis consumer: isolated work

This branch starts from the statically reviewed, unhooked `04c1567` checkpoint.
The first new checkpoint contains a real Engine reftest and no layout change.
No Cargo, SSH, CI or local browser execution has been performed for this branch.
It is not a stable delivery and does not claim that a disconnected helper is
operational.

The test/reference HTML files are byte-identical copies of the previously
frozen `ec9cee0` pair `wrapper-definite-perpendicular`. Five adjacent physical
10x20px inline blocks, with zero font/line height, yield five columns in a
30px perpendicular span and two columns in a 60px span. The reference uses
ordinary Block boxes of physical widths 50px/20px, not Grid or Flex sizing.
Clockwise, reflected and counter-clockwise wrappers exercise the 30px case.

The original pair's official 21d and 3c baseline executions both failed with
2800 differing pixels; those receipts and PNGs remain in
`grid-estimate-review-2026-11-14/v2`. That historical result is not a new
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
