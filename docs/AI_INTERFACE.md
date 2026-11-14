# RabbitHole for AI agents

RabbitHole can be driven by LLM agents over **MCP** (`rabbithole mcp`, JSON-RPC
2.0 on stdio) or from a shell (`rabbithole agent`). Both use the same twelve
tools, implemented in `crates/agent`. This document is the specification of
that interface: the tools, the snapshot grammar, the auto-wait and diff
semantics, the error messages, and the safety rules.

Design goals, in order:

1. **Cheap in tokens.** A compact, accessibility-tree style text snapshot —
   never nested JSON, never raw HTML, never a screenshot per step.
2. **Stable handles.** Elements are addressed by refs (`@e12`) that stay valid
   while the element exists, across snapshots.
3. **Actions that wait and explain.** Every action auto-waits until the page
   settles and returns *what changed*; failures say why and what to do.
4. **Safe by default.** Page content is fenced and escaped so it can never
   look like tool output; hidden text is dropped; downloads are not saved;
   permissions are denied; cookie banners are rejected, never accepted.
5. **One identity.** Programmatic use presents the same standard browser
   identity as the GUI; the honest `RabbitHoleBot` identity with `robots.txt`
   enforcement is an explicit opt-in (see [Identity](#identity-and-robotstxt)).

## Tools

| Tool         | What it does |
|--------------|--------------|
| `open`       | Load a URL (or `back` / `forward` / `reload`); default: short [page summary](#the-page-summary). `find` (string or string array) returns the header, covering-layer warnings and matching passages; `window` and `limit` adjust that search, and `summary=true` adds the overview. Without `find`, `snapshot=true` (alias `full=true`, or any `max_tokens`) returns the full snapshot. Other options: `new_tab`, `context` (isolated-session name), `bot`, `robots`, `allowlist`, `javascript`, `dialog`. |
| `snapshot`   | The semantic tree of the page. `ref` expands one element (a truncated list, a collapsed landmark); `max_tokens` (default 2500); `hidden=true` also lists hidden content, marked; `urls=true` shows link targets. |
| `read`       | Main content as Markdown, without nav/footer/related; without `ref`, starts at the article beginning. Optional `ref` reads that subtree. `max_tokens` defaults to 4000; `offset` counts characters. `links=true` keeps link refs; `links=false` uses plain text. `cites=false` omits same-page superscript citation links; `cites=true` includes them. Use `find` for a particular fact. |
| `find`       | Search `text` (string or string array, any term), `role` + `name`, or CSS `selector`. Returns element refs in document order. `window` (default 240 characters; `context` is an alias) selects passages around the first occurrence of each term, capped at three windows per element. Zero keeps the legacy first 120. `limit` counts elements (default 20). |
| `click`      | Click by ref (real hit-tested pointer sequence). `hover=true` only hovers. `force=true` dispatches even if covered/invisible. `ref="cookies"` rejects a cookie banner. |
| `type`       | Type `text` into a field with per-key events (`clear=true` replaces), and/or press a `key` (`Enter`, `Tab`, `Escape`, `ArrowDown`, `Backspace`, …). `submit=true` presses Enter afterwards (implicit submission). |
| `fill`       | No arguments: list forms. `ref=@fN`: list its fields. `values={label|name|id|@eN: value}` fills text, checkbox booleans and select/radio options. Without `ref`, use only `@eN` keys or a page with one form. Stops remaining fields after navigation or document replacement. `submit=true` validates and submits. A single field: `ref=@eN value=…`. |
| `scroll`     | `direction` (`down`/`up`/`top`/`bottom`, `pages`), `ref` (scroll into view), `until=@eN` or `until="text"` (scroll until visible), `load_more=true` (scroll to the end until the page stops growing, clicking a "load more" button when found). |
| `screenshot` | PNG of the viewport, the `full` page or one element (`ref`), with set-of-marks: every visible interactive element boxed and labelled with its ref, plus a text legend. Only on demand. |
| `network`    | The tab's requests: `#id METHOD STATUS type content-type size time path`, with API hints. `id=N` shows a response body (JSON pretty-printed). `filter`, `type`, `all` (include images/fonts/CSS), `limit`. |
| `tabs`       | `action=list|switch|new|close` tabs (popups appear here), `download` (save an offered download to `path`), `save`/`load` a session (cookies, localStorage, open tabs) to/from a directory, `context` for isolated contexts. |
| `debug`      | `eval` JavaScript, returned Promises or top-level `await`; optional `timeout` in milliseconds and `follow=true` for script navigation. `console=true`, `clear`, `log=script|json`, `live=true`. |

Every action (`open`, `click`, `type`, `fill`, `scroll`) accepts `dialog`:
the answer to `alert`/`confirm`/`prompt` raised during that call — `accept`,
`dismiss` (the default) or the text to answer a prompt with.

The tool-budget test checks the compact `tools/list` result object,
`{"tools":[…]}`, at at most 6,900 Unicode scalar values, with each tool description
under 200 UTF-8 bytes. This is a catalogue-size gate, separate from model tokens,
page output and the surrounding JSON-RPC envelope.

### CLI

`rabbithole agent` exposes the same tools:

```sh
rabbithole agent -c "open https://en.wikipedia.org/wiki/Rabbit" -c "read max_tokens=800"
rabbithole agent --ephemeral            # REPL: one command per line on stdin
> open https://news.ycombinator.com/
> click @e14
> type @e5 "rabbits" submit=true
> fill @f1 values={"email": "ana@example.test", "Remember me": true} submit=true
> screenshot path=page.png
> debug log=json
rabbithole agent --script session.txt   # replay commands
rabbithole agent --replay test.json     # replay an exported test, checking URLs
```

Syntax: `<tool> [positional…] [key=value…]` (values are parsed as JSON when
possible) or `<tool> {json arguments}`. Positional arguments: `open URL`,
`click REF`, `type REF TEXT`, `fill REF`, `find TEXT`, `scroll DIRECTION|REF`,
`network ID`, `tabs ACTION ID`, `debug JS…`, `snapshot REF`, `read REF`.

Options shared with `rabbithole mcp`: `--bot`, `--no-robots`,
`--ephemeral` (in-memory profile; the default profile is `<profile>/agent`),
`--profile DIR`, `--allow d1,d2` (domain allowlist), `--live`, `--timeout MS`,
`--width`/`--height`, `--no-js`, `--images`, `--system-fonts`.

## Lighter read output

Without `ref`, main-content selection uses the live, filtered DOM. It scores
prose, including containers made of inline children, and excludes navigation,
footers, forms, controls and hidden content. A small `main` or `article` shell
does not hide substantial prose elsewhere: when no suitable container holds
enough of the text, the reader uses the filtered body. `read ref=@eN` keeps the
requested subtree rather than running that selection again.

The published reading contract keeps `links` set to
`true`, retaining `[text](@eN)` and stable navigation refs. `links=false` emits
plain anchor text without assigning link refs. `cites` defaults to `false`:
anchors inside a `<sup>` that resolve to a fragment of this same document are
omitted. `cites=true` restores those marks; it does not override `links=false`.
Other superscripts, external links and links to another document are retained.
The document base URL participates in resolving citation targets.

Prose whitespace, including nonbreaking spaces, is collapsed, and whitespace
before `, . ; : )` is removed. Code spans and fenced preformatted content retain
meaningful spaces and blank lines; code containing backticks uses a longer
Markdown delimiter. List indentation, link ref syntax and page-content fences
remain intact. The existing `max_tokens=4000`, character `offset`, and `ref`
subtree behaviour are unchanged. Benchmark results are tracked separately with
their measured source revision and conditions.

## The page summary

`open`, and every action that navigates (a clicked link, a submitted form),
return a short summary of the page instead of the full snapshot, within a few
hundred tokens even on big pages:

```
⟦page 3f9a1c2b⟧
[page] "Portada - Example News"  https://news.example.test/  status=200  scroll=0/4180
⚠ [dialog] @e200 covers the page (62% of the viewport): "Rechazar" @e201, "Aceptar todo" @e202
landmarks: [header] @e1  [nav "Secciones"] @e2  [main] @e9  [footer] @e99
outline:
  h1 "Portada" @e8
    h2 "Título de la noticia 1" @e10
    h2 "Título de la noticia 2" @e14
  … +21 more headings
forms:
  @f1 form "Buscar" (GET /buscar): textbox "Buscar" @e6, button "Ir" @e7 (submit)
  inputs outside forms: combobox "Sección" @e12
main @e9: 120 links, 8 buttons, 1 inputs; whole page: 310 links, 14 buttons, 2 inputs
⟦/page 3f9a1c2b⟧
An overlay @e200 covers the page: deal with it first (click ref="cookies" rejects cookies).
Summary only: open … find= or find text= locates facts with context; read without ref starts at the article beginning; snapshot shows the full tree.
```

* Overlays and dialogs come first (`⚠`), with their buttons.
* `landmarks`: up to 10 groups with refs; `main` and `search` have priority.
  Repeated navigation landmarks share one `nav ×N` entry retaining the first ref
  (use `snapshot ref=@eN` to expand that navigation).
* `outline`: up to 12 headings, prioritising the two highest heading levels
  present before deeper headings. Selected headings remain in document order,
  indented by level, each with a ref (the link's ref when it is a link).
* With at most two headings and at least 200 characters of selected main
  content, `text: N chars in main (read shows it)` signals that prose is available.
  `N` counts Unicode scalar values after normalising whitespace and nonempty
  text-node boundaries to one space. It does not count Markdown, refs or an
  added page title, and does not insert the article into the summary.
* `forms`: up to 3 forms that contain a text field or submit control, with up to
  6 fields each (label, type, ref, submit button), plus text inputs outside forms.
  Other forms are counted as `… +N formularios sin campos de texto`; `fill` still
  lists all forms and fields.
* Counts of links, buttons and inputs in `main` and in the whole page.

The refs are the same ones `snapshot`, `find` and the actions use.
`open url=… snapshot=true` (or the `snapshot` tool) gives the full tree.

## Finding facts

Search during navigation to locate a fact in one call:

```json
{"url":"https://example.test/article","find":["Guido","1989","CWI"],"window":240,"limit":5}
```

`open` with `find` preserves the page header, navigation notices and covering-layer
warnings, then returns search results. It omits landmarks, outline, forms and
counts unless `summary=true`. The header, warnings and passages share one
untrusted-content fence. Search uses the default 240-character window and
20-element limit. `find` takes precedence over snapshot options; ordinary
`open` and `snapshot` output keep their existing behaviour.

On `open`, `context` is exclusively an isolated-session name, not a search
budget. Numeric values and names containing only ASCII digits are rejected
before navigation or context creation; use `window` for the passage size.

On a loaded page, `find {"text":["Guido","1989","CWI"],"limit":10}` searches
all eligible elements once, returning an element when any term matches. Results
follow document order, including child documents at their iframe. Multiple text
runs belonging to one element count as one result. `role` and `name` restrict the
eligible elements before matching. CSS selector search keeps its existing format.
Search excludes hidden content, as before.

Inline links contribute their complete labels to searchable prose: a paragraph
`Guido <a>CWI</a> 1989` matches `Guido CWI 1989`, never `Guido 1989`. Search builds
this plain text with the normal tree's visibility rules and node refs; it does
not parse snapshot link markers or change snapshot rendering. Other separate
semantic runs keep their boundaries. Hits in different runs of one element
share a result and can contribute separate fragments to its snippet.
Linked prose uses its real common containing element, including across inline
spans, so the result ref can read the complete passage.

```text
@e17 text "…Guido began Python in December 1989 at CWI…" (below viewport) [text: "Guido", "1989", "CWI"] (+2 more in this element)
No matches for: "diciembre de 1989"
```

The core result grammar stays `@eN role "snippet" (position)`, with an optional
position; text matches append `[text: "term", …]` in request order. Occurrences
not wholly visible in the returned fragments append `(+N more in this element)`.
This includes a term whose window did not fit; its presence is still reported.
Terms are case-insensitive literals; duplicate terms
that lowercase identically are combined. There is no regex option. Empty searches
and arrays containing nonstrings are errors.

`window` is an integer from 0 through 4096. On `find`, `context` remains an
alias; `window` wins when both are present. A positive window centres on the
first occurrence of each requested term. Overlapping or touching windows are
merged, and separate fragments are joined with `…` in source order. The snippet
for **each element**, including these ellipses, is capped at `3 × window` original
Unicode characters before display escaping. Terms are admitted in request order
when the cap prevents adding another fragment. Small windows can clip a match.
Lowercase expansions such as `İ` map back to original character positions.
`window=0` is the explicit exception: it preserves the old first-120-character
clipping. Quotes, backslashes and control characters retain their escaping rules.

Text search without an explicit `role` can omit redundant links whose normalised
label exactly equals a requested term when matching prose is also available.
It reports `(N redundant links omitted; use role=link to include)`. An explicit
role search retains its usual matching elements; a distinct link label is kept.

`limit` truncates returned elements, not the scan: a term found later is never
listed as absent just because its element was omitted. The final
`No matches for:` line reports terms absent from the entire eligible selection.
Search results and this line remain inside the untrusted-content fences; refs
remain usable with `read`, `snapshot` and actions.

## The snapshot

```
⟦page 3f9a1c2b⟧
[page] "Portada - Example News"  https://news.example.test/  status=200  scroll=0/4180
[dialog] @e200  ⚠ covers the page (62% of the viewport)
  text "Usamos cookies para mejorar tu experiencia."
  button "Rechazar" @e201  button "Aceptar todo" @e202
[main] @e9
  heading(2) "Título de la noticia 1" @e10 → link
    text "Resumen corto de la noticia uno, con detalles."
    button "votar" @e11  text "342 votos"
  heading(2) "Título de la noticia 2" @e14 → link
  link "Elemento número 1" @e20
  link "Elemento número 2" @e21
  … +23 more items (18 below the viewport), expand @e30
[header] @e1
  [nav "Secciones"] @e2
    link "Portada" @e3  link "Nuevas" @e4  link "Populares" @e5
    textbox "Buscar" @e6
[footer] @e99 collapsed (34 links, 2 texts)
⟦/page 3f9a1c2b⟧
Hidden content not shown: 412 chars of text, 3 controls (…). Use hidden=true to list it marked.
```

### Grammar

```
snapshot   := fence-open NL header NL { line NL } fence-close { NL note }
fence-open := "⟦page " NONCE "⟧"           ; NONCE: 8 random hex digits per response
fence-close:= "⟦/page " NONCE "⟧"
header     := "[page] " QUOTED "  " URL "  status=" INT [ "  scroll=" INT "/" INT ] [ "  frames=" INT ]
line       := INDENT ( section | items | more )
INDENT     := { "  " }                      ; nesting: sections, then heading outline
section    := "[" label [ " " QUOTED ] "]" [ " " REF ] { " " state } [ " src=" URL ] [ "  ⚠ " overlay-note ]
            | "[" label [ " " QUOTED ] "] " REF " collapsed (" summary ")"
label      := "main" | "nav" | "header" | "footer" | "aside" | "search" | "region"
            | "form" | "dialog" | "group" | "iframe" | "figure" | "article" | …
items      := item { "  " item }            ; items on the same visual row share a line
item       := "text " QUOTED
            | "img " QUOTED
            | "heading(" LEVEL ") " QUOTED [ " " REF " → link" ]
            | role [ "(h" LEVEL ")" ] [ " " QUOTED ] [ " " REF ] { " " state } [ " → " URL ]
            [ " (hidden: " reason ")" ]     ; only with hidden=true
more       := "… +" INT " more items" [ " (" INT " below the viewport)" ] ", expand " REF
REF        := "@e" INT                      ; forms: "@f" INT (in fill output)
QUOTED     := '"' escaped-text '"'
state      := "checked" | "unchecked" | "mixed" | "disabled" | "expanded" | "collapsed"
            | "selected" | "pressed" | "current" | "required" | "readonly" | "invalid"
            | "focused" | "password" | "type=" WORD | 'value="' text '"'
            | "value=(" INT " chars hidden)" | "options=[" a "|" b … "]" | INT " options"
            | "selected=[" … "]"
role       := "link" | "button" | "textbox" | "searchbox" | "checkbox" | "radio"
            | "combobox" | "listbox" | "slider" | "spinbutton" | "switch" | "tab"
            | "menuitem" | "option" | "treeitem" | "progressbar" | "meter"
            | "clickable"                   ; element with a JS click listener/onclick
```

`escaped-text` never contains `"` (replaced by `'`), backslashes, newlines,
control characters, bidi overrides, or the fence brackets `⟦` `⟧` (replaced
by `|`). Page strings therefore cannot terminate a quote, start a new line,
or close the fence.

### What is included

* **Roles** come from ARIA (`role=`) or the implicit HTML role (HTML-AAM):
  links, buttons, form controls, headings, landmarks (`main`, `nav`, `header`
  → banner, `footer` → contentinfo outside sectioning content, `aside`,
  `search`, named `section` → region, `form`), dialogs, lists, tables,
  images with alt text, iframes. Layout `div`s and classes never appear.
* **Accessible names** follow accname 1.2 in practice: `aria-labelledby`,
  `aria-label`, `<label for>` / wrapping `<label>`, `alt`, `<legend>`,
  `<caption>`, `<figcaption>`, button `value`, name-from-content for links,
  buttons, headings, tabs, options…, then `title` and `placeholder`.
* **Inline state** as listed in the grammar; values of password fields are
  never shown.
* **Text** that is actually painted. Text is dropped (and counted in the
  footer note) when it is `display:none`, `visibility:hidden`, `opacity:0`,
  zero-sized, clipped away by `overflow`, off-screen (negative coordinates),
  in a tiny font, `aria-hidden`, or drawn in (nearly) the background color
  (contrast < 1.12 against the effective background). `hidden=true` lists it
  marked `(hidden: reason)`.
* **Order:** dialogs and overlays first, then `main`, then the other sections
  in document order. Pages without `<main>` get one inferred from
  Readability scoring. Within a section, content after a heading is indented
  under it (outline).
* **Overlays** are found with real hit testing: fixed/raised boxes sampled on
  an 8×8 grid of viewport points. One that is topmost on ≥ 25% of the points
  (or is a cookie/consent banner, or an open dialog) is shown as `[dialog]`
  first with `⚠ covers the page (N% of the viewport)` or `⚠ cookie banner`.
* **Lists** (`ul`/`ol`/tables/`role=list`… and runs of ≥ 6 similar siblings
  such as cards or feed entries) are truncated to the first items, with
  `… +N more items (M below the viewport), expand @eX`; `snapshot ref=@eX`
  shows the whole list.
* **iframes** (same- and cross-origin, `srcdoc`, up to depth 3) are loaded,
  run, and inlined under `[iframe "title"]` with normal refs; clicks inside
  them work. **Shadow DOM** content (script `attachShadow` and declarative
  `<template shadowrootmode>`) appears inline too.

### Budget

`max_tokens` (≈ characters / 4, default 2500) is met by lowering detail step
by step: shorter lists (12 → 2 items), shorter text items (400 → 50 chars),
then collapsing `footer`, `aside`, `nav`, `header`, other regions (in that
order) into one-line summaries `[footer] @e99 collapsed (34 links, 2 texts)`.
`main` is never collapsed. As a last resort the output is cut with a note.

### Refs

* A ref names one DOM node of one frame for as long as that node is in the
  document. Re-snapshots and actions keep it.
* Refs are never reused within a tab. After a navigation, old refs fail with
  *"@e11 (button "votar") belongs to a previous page; the tab has navigated
  since. Take a new snapshot."*
* If the node was removed (re-render), the error suggests the most similar
  current element (same role and name, then same name):
  *"@e11 (button "votar") no longer exists (page re-rendered). Similar
  candidate: @e87 button "votar""*.
* Forms have `@fN` refs (see `fill`). `find` and `read` assign refs to what
  they return, so anything can be targeted.

## Actions: auto-wait and diffs

After `open` and every action the page runs until **settled**, capped by
`--timeout` (default 5 s):

* no request in flight that blocks (documents, scripts, `fetch`/XHR; images,
  fonts, WebSockets and EventSource streams excluded),
* no timer due within 600 ms,
* and when only timers keep firing (animations, polling), the DOM unchanged
  for 400 ms.

If the cap is hit, the result says what was still pending:
*"Clicked @e5 button "Cargar" (not settled after 5.0 s: waiting for GET
/api/feed (4.8 s))"*.

The result of an action is:

* **Navigation:** `Clicked @e3 link "Nuevas" → navigated to URL (status 200)`
  followed by the [page summary](#the-page-summary) of the new page (call
  `snapshot` for the full tree).
* **Same page:** a diff of the semantic tree, fenced:

  ```
  Clicked @e11 button "votar"
  ⟦page 51c0…⟧
  ~ button "votar" @e11 ⇒ button "votado" @e11 disabled
  ~ text "342 votos" ⇒ text "343 votos"
  + text "¡Gracias por votar!"
  - button "deshacer" @e15
  ⟦/page 51c0…⟧
  ```

  `~` changed (same node), `+` added, `-` removed; at most 40 lines. When
  most of the page changed, a new snapshot is returned instead.
* **Only the look changed** (a class toggle, a highlight): the painted
  viewport before and after is compared —
  *"No content change, but the page changed visually: 1 region(s), 20000 px²,
  largest 200x100 at (8, 8) in the viewport (a style-only change; screenshot
  to see it)."*
* **Nothing happened:** never silent —
  *"⚠ The action had no visible effect: no DOM change, no navigation, no
  event listeners on the target."*
* **Events** (below) are appended to any result.

Clicks scroll the element into view and hit-test several points of its box;
the pointer sequence (`pointerover`… `mousedown`, focus, `mouseup`, `click`,
activation behaviour) is dispatched at the first point where the element is
topmost. `type` dispatches `keydown`/`keypress`/`input`/`keyup` per
character (respecting `maxlength` and handlers that rewrite the value) and
`change` at the end. `Enter` in a text field performs implicit form
submission through the default button.

## Errors

Errors are sentences an agent can act on:

| Situation | Message |
|-----------|---------|
| Covered   | `@e11 button "votar" is not clickable: covered by @e200 (dialog 'Aceptar cookies'). Close or dismiss it first (click ref="cookies" rejects cookies), or pass force=true.` |
| Stale ref | `@e11 (button "votar") no longer exists (page re-rendered). Similar candidate: @e87 button "votar"` |
| Old page  | `@e11 (…) belongs to a previous page; the tab has navigated since. Take a new snapshot.` |
| Invisible | `@e7 link "Menú" is not visible (no box, zero size or hidden). Pass force=true to dispatch the event anyway.` |
| Disabled  | `@e9 button "Enviar" is disabled.` |
| HTTP      | `Navigation to URL failed: HTTP error 403. The site refused the request (login required or bot blocked).` |
| Bot wall  | note `HTTP status 403: the page looks like a captcha/bot-check wall; an automated agent cannot pass it.` |
| Login     | note `Page requires login (form detected at @f1; fill ref=@f1 to see its fields).` |
| robots    | `Navigation to URL failed: blocked by robots.txt: URL is disallowed for RabbitHoleBot …` |
| Allowlist | `Navigation blocked: host is not in the domain allowlist.` |
| Timeout   | `… (not settled after 5.0 s: waiting for GET /api/feed (8.0 s))` |
| Validation| `Not submitted: 2 invalid field(s):` + one line per field: `@e5 "Email": required (empty)`, `@e6 "Usuario": does not match the required pattern (solo minúsculas)` |

## Forms

`fill` without arguments lists every form:

```
@f1 form "Entrar" (POST /session)
  textbox "Email" @e5 name=email type=email required
  textbox "Contraseña" @e6 name=password type=password required
  checkbox "Recordarme" @e7 name=remember
  combobox "Idioma" @e8 name=lang value="Español" options=[Español|English]
  button "Entrar" @e9 (submit)
```

Keys of `values` match, in order: a ref (`@e5`), the `name`, the `id`, the
label (case- and whitespace-insensitive), the placeholder, then a substring
of the label. Radio groups are addressed by group name with the option's
label or value.

Without `ref`, a map containing only `@eN` keys resolves each field directly;
the fields may belong to different forms or frames. With `submit=true`, they
must share one form. A map using names, labels or a mixture of key types needs
one unambiguous form in the tab; if there are several, the error lists them
so the next call can supply `ref=@fN`.

Each field waits for its focus/input/change work before the next field. A
navigation requested by focus stops before assigning the value; one requested
by input stops before dispatching change. Navigation or document replacement
during any of these steps, including settling timers or responses, stops the
remaining fields and explicit submit. The result reports what was not attempted
and requests a new snapshot. A document replacement invalidates that frame's
old refs even if the same node was reattached. A navigation uses the same
navigation result and page summary as `click`; remaining fields need fresh refs.

`submit=true` runs HTML constraint validation (`required`,
`type=email|url|number`, `min`/`max`, `minlength`/`maxlength`, `pattern`
evaluated with the page's own RegExp engine; `novalidate` respected); invalid
fields are reported and marked `invalid` in later snapshots. Valid forms are
submitted by clicking the default submit button (so page handlers run), or
by firing `submit` when there is none. Cancellation by the click or submit
handler is reported as `Not submitted`.

## Network

```
#3 GET 200 document text/html 14.2 KB 180 ms /
#7 GET 200 fetch application/json 3.4 KB 95 ms /api/posts?page=2
API hint: the new content (12 items) comes from GET /api/posts?page=2 — read it as JSON with network id=7
```

Bodies of API calls, documents and textual responses up to 2 MB are kept
per tab. **API hints** correlate string values of JSON responses with text
that appeared in the DOM after an action (or is on the page), so the agent
can read the data directly instead of scraping it.

## Events

Script dialogs, popups, downloads and new JS errors are reported after the
result of the call that caused them:

```
Events:
- Dialog confirm("¿Borrar?") → dismissed (returned false; repeat the action with dialog=accept to accept)
- New tab t2 opened: https://example.test/popup (switch with tabs action=switch id=2)
- Download #1 offered: "datos.zip" (1.2 KB, application/zip) from … — not saved. Save it with tabs action=download id=1 path=FILE.
- 2 new JS error(s), last: "TypeError: x is undefined" (debug console=true)
```

Dialogs are answered by policy, synchronously as scripts require: `alert`
is dismissed, `confirm` returns `false` and `prompt` returns `null` unless
the call passed `dialog=accept` (or a prompt answer). Downloads are never
written to disk unless `tabs action=download` asks for it.

## Sessions and contexts

* The default context uses a persistent profile (`<profile>/agent`: cookies
  and localStorage); `--ephemeral` keeps it in memory.
* `open context=NAME` (or `tabs action=new context=NAME`) creates an isolated
  context: its own cookie jar and localStorage. Tabs of different contexts
  can be used side by side.
* `tabs action=save path=DIR` writes cookies (Netscape format), localStorage
  and the open tab URLs; `tabs action=load path=DIR context=NAME` restores
  them into a context (tabs load on first use).

## Safety

* **Delimiting.** All page-derived content (snapshots, diffs, read output,
  `find` results, network bodies, console, eval results) is enclosed in
  `⟦page NONCE⟧ … ⟦/page NONCE⟧` with a fresh random nonce per response. Page
  text cannot contain the fence brackets (they are replaced), and inside the
  snapshot every page string is quoted and escaped. Tool messages (errors,
  notes, events) are outside the fence. The MCP `instructions` tell the model
  that fenced content is untrusted data.
* **Hidden text** is removed by default (see above), because what is not
  painted is a classic prompt-injection channel.
* **Permissions** are denied: `navigator.permissions.query` answers
  `denied`, `Notification.permission` is `denied`, `getUserMedia` rejects,
  geolocation never delivers a position.
* **Domain allowlist:** `--allow` or `open allowlist=[…]` restricts
  navigations and iframes.
* **Cookie banners:** `click ref="cookies"` looks for a reject / decline /
  "only necessary" button (in several languages) in the detected banner and
  clicks it; if there is none it fails and asks the agent to choose. It never
  accepts.
* **Downloads** are reported, not saved.

## Identity and robots.txt

**By default every mode — the GUI, the MCP server, the CLI subcommands
(`rabbithole js|text|links|screenshot|diff|agent…`) and the programmatic API
(`rabbithole_agent::Config::default()`) — sends the same standard browser
headers** (`HeaderProfile::Browser`: a Chrome-compatible User-Agent and client
hints, as every modern engine sends), and does not consult `robots.txt`.

The bot identity is an explicit opt-in:

```
User-Agent: Mozilla/5.0 (compatible; RabbitHoleBot/0.1.0; +https://github.com/i234dev/RabbitHoleBrowser) RabbitHole/0.1.0
```

with no Chrome client hints (`sec-ch-ua*`), and it respects `robots.txt`
(fetched once per origin and cached; the `RabbitHoleBot` group if present,
otherwise `*`; longest match wins, `*` and `$` supported) for top-level
navigations. A blocked URL fails with a clear `blocked by robots.txt` error.

This is a `crates/net` setting: `Fetcher::set_header_profile(HeaderProfile::Bot
| HeaderProfile::Browser)`, plus `Fetcher::respect_robots`. Opt-ins: `--bot`
(CLI subcommands, `rabbithole agent`, `rabbithole mcp`), `open bot=true` (MCP
session option; `bot=false` switches back), `Config { bot: true, .. }` (API).
`--no-robots` / `open robots=false` keep the bot identity without robots
checks. The legacy `--browser-headers` / `browser_headers=true` spellings are
still accepted and mean the default.

## Debugging and reproducibility

* `debug console=true` shows console output and JS errors; new errors are
  also announced as events.
* `debug eval="…"` evaluates JavaScript in the page and observes a returned
  Promise. Top-level `await` is detected from syntax; there is no `await` flag.
  Await-enabled evaluations have invocation-local bindings; ordinary evaluations
  keep their existing global behavior. Module imports/exports are unsupported.
  Resolved values are inspected, rejections report `Uncaught (in promise) …`,
  and a Promise still pending at the deadline reports
  `Promise still pending after N ms`.
* `timeout=N` is a positive integer in milliseconds, defaulting to the configured
  auto-wait cap. It limits JS execution and observation, including VM work,
  microtasks, callbacks and ready-response polling. It does not wait for an
  unfinished background HTTP request to complete. Existing synchronous native
  I/O cannot always be interrupted, so this is not a wall-clock guarantee for
  arbitrary page code.
* `follow=false` is the default: script navigation, dialogs and new windows are
  reported without following them. `follow=true` uses the normal `click` action
  handling after evaluation, including form POSTs and navigation errors. That
  navigation has its own network/settling limits and can outlast the JS timeout.
* Every mutating call is logged. `debug log=script` exports it in CLI syntax
  (replay with `rabbithole agent --script FILE`); `debug log=json` exports a
  test (`{"steps":[{"tool","args","expect_url"}]}`) that `rabbithole agent
  --replay FILE` runs and checks.
* `debug live=true` (or `--live`) starts a local HTTP page that shows the
  agent's tab, re-rendered after every action, so a human can watch.

## Performance

Historical observations on the development machine (release build, Linux
x86-64, while other builds were running). Interface changes do not refresh
these measurements. Rows without a source revision or date are historical
observations, not measurements of the current version:

| What | Result |
|------|--------|
| `rabbithole mcp` start → `initialize` reply → exit | 17–31 ms (the browser is created lazily on the first tool call) |
| Start + first `open` of a small page + exit | ≈ 1 s wall, 0.2 s CPU (bundled fonts only; system fonts load on the first screenshot) |
| Resident memory, 1 / 10 / 50 isolated contexts with one small page each | 115.9 / 116.3 / 117.2 MB — about 25 KB per extra context; the ≈ 114 MB base is the engine (fonts, script runtime) |

Token cost of one page (approx. characters / 4), old MCP `dom` and `browse`
tools versus the new `snapshot` (default budget) and `read`:

| Page | old `dom` | old `browse` (text) | `snapshot` | `read` |
|------|----------:|--------------------:|-----------:|-------:|
| en.wikipedia.org/wiki/Rabbit | 393,830 | 45,101 | 1,269 | 4,242 (first 4,000-token page) |
| news.ycombinator.com | 14,253 | 3,572 | 628 | 1,096 |
| github.com/rust-lang/rust | 100,573 | 3,273 | 1,483 | 1,166 |
| bbc.com/news | 62,706 | 4,659 | 2,304 | 2,648 |
| html.duckduckgo.com (results page) | 7,304 | 2,336 | 874 | 1,193 |

`open` returns the [page summary](#the-page-summary); the full snapshot is
opt-in (whole tool output, fences and notes included; measured 2026-11-11):

| Page | `open` (summary) | `open snapshot=true` |
|------|-----------------:|---------------------:|
| en.wikipedia.org/wiki/Rabbit | 252 | 1,243 |
| news.ycombinator.com | 74 | 498 |
| github.com/rust-lang/rust | 325 | 1,577 |
| bbc.com/news | 221 | 1,927 |

Page load time is dominated by the engine (script execution and full
restyles on large pages); the auto-wait adds at most its cap.
