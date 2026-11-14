# Document font views

`FontContext::document_scope()` creates a fresh family and fallback view over a
shared font store. It inherits the live host catalog (bundled fonts, system
fonts and explicit host registrations), without inheriting another document's
web-font aliases. Register each document's `@font-face` declarations in its own
view. Selection and fallback inspect only that view and the host catalog.

The store assigns append-only `FontId` values. A retained document can continue
to use its IDs when another document is loaded. Rasterized glyphs, color glyphs
and shaping data remain shared by stable face ID. Fallback results belong to the
view; adding a host face invalidates cached fallback misses in existing views.

Decoded SFNT bytes are interned by a hash followed by exact byte comparison.
Identical bytes share the parsed font and SFNT allocation across aliases. A
binding additionally includes normalized family name, CSS weight and italic
state; those descriptors must not be borrowed from a different alias. Repeated
registration of the same binding returns its existing ID. System paths retain
their existing lazy-loading behavior.

`FontLimits` configures retained web resources per shared store. Defaults are
4096 web bindings and 128 MiB of unique additional decoded SFNT data. Bundled
and system font catalogs do not consume these quotas. Existing interned bindings
remain usable when a quota is exhausted. A rejected registration creates no
alias and consumes no quota. Quotas persist for the store's lifetime: they are
not cleared when a document is released, because existing IDs may still be used
by retained pages, display lists or canvases.

These quotas bound retained bindings and additional SFNT bytes, not the whole
heap. Container decoding uses temporary allocations; parsed font structures,
encoded resource caches, shaping caches and glyph bitmaps have separate memory
costs. The SFNT quota is not a decoder scratch or glyph-cache budget. Arbitrarily
many distinct navigations can exhaust the store's registration quota; subsequent
new fonts then use the normal unsuccessful-registration fallback. Reusing an
existing binding still succeeds.

## Page integration

Each `Page` owns its document view. `Engine.fonts` remains the clean host view
for browser chrome and explicit host registrations such as Ahem. Layout,
inline SVG text, complete and damaged painting, frame rasterization and Canvas
text use the page view. Content GUI caret/selection/find, PDF pagination and
screenshot diagnostics pass that same view when reshaping display-list text.
Standalone callers painting a page's display list must also pass `page.fonts`.

Incremental style/layout observes the document and host selection revisions.
A newly available local or host face invalidates the page's style and layout on
its next flush. Frame synchronization also notices a child's changed font view
and regenerates its cached raster. Screenshot setup restyles after loading
system fonts, before it calculates capture geometry. Loading another page never changes the aliases of
a retained page or frame.

Font-relative CSS units use a scoped thread-local guard. It saves and restores
fonts, the metric cache and unicode-range exclusions, including nested passes
and unwinding. Cache keys use constant-time selection revisions rather than
enumerating the font catalog. Resource discovery without a Page enters an empty
font scope; full/incremental passes, unrendered style queries, verification and
animation keyframes enter their document's scope. Layout retains a guard too,
including text-autospace measurements that occur after styling. An unchanged style flush
returns before collecting font descriptors or entering a new scope.

## Canvas font selection

Canvas `measureText`, `fillText` and `strokeText` resolve the same parsed CSS
font shorthand through the owning document's view. Family entries retain their
order. A quoted name such as `"serif"` is a literal registered name; unquoted
`serif` selects the generic family. Missing named entries fall through to the
next family, while a generic entry selects the host's generic immediately.
The font setter keeps its previous value when shorthand resolution fails.

The shared shorthand resolver takes an explicit parent and length context.
Canvas currently supplies its fixed 16px relative/root base, zero viewport
metrics and fixed relative glyph metrics (ch/ex=0.5, ic=1). It does not derive
these from the canvas element's computed font or another document's TLS.
This change addresses named-family selection, without adding element-relative
font inheritance or claiming full CSS font-shorthand conformance.
