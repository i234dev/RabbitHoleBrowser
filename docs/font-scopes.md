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
