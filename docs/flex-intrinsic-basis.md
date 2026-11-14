# Intrinsic preferred main sizes in Flex

This successor of Grid candidate `ae5bdeaa1ffd62b8c4a5b0afb292e4120e987aa6`
is **SIN VALIDAR**. No Rust build, test or reftest has run for this delta.
The original candidate and its failed 26-case gate remain unchanged.

Three row-lanes alignment comparisons exposed a Flex reference defect:
`flex-basis:auto;width:min-content` used the max-content base, while the
Grid item correctly used min-content. This change fixes the Flex rule rather
than reverting the Grid sizing or modifying the WPT references.

The computed style preserves `content` separately from `auto`, through
shorthands, resets and CSS-wide copies. `content` continues to request content
sizing even with a specified preferred main size. For `auto`, the used basis
retrieves the preferred main property, including its intrinsic keyword.
The row axis reads width and the column axis reads height. Fit-content uses
the available main space after the item's margins, padding and border.
Numeric resolution and the existing indefinite-percentage fallback are kept;
the automatic minimum and later minimum/maximum clamps remain separate.

Sources: [Flex §7.2.3](https://drafts.csswg.org/css-flexbox-1/#valdef-flex-basis-auto),
[Flex §9.2](https://drafts.csswg.org/css-flexbox-1/#algo-main-item), and
[Sizing §2.1](https://drafts.csswg.org/css-sizing-3/#fit-content-size).

Controls cover row/reversed row intrinsic keywords, content versus auto,
explicit bases, fit-content edge subtraction, definite sizes/percentages,
column/reversed column height keywords and unresolved percentage bases,
automatic minima and numeric clamps. A visual pair uses independent block
references, including a wrapped word, a reversed row and column heights.
Existing replaced/ratio/writing-mode measurement paths are reused; this delta
does not expand their sizing algorithms or repair general Flex intrinsic
container sizing. Those existing tests and a wider frozen Flex WPT subset
must accompany the Grid gates before integration.

The orthogonal Grid subgrid failure is a separate unresolved gate. This
Flex change does not make the Grid candidate integrable by itself.
