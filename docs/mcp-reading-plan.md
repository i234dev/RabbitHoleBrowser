# MCP reading improvements

Status: planned. Implement against the current development revision when this
milestone starts; recheck the code and tool catalogue at that time. The source
revision used by the original investigation is historical, not an implementation
base. This work follows the current compatibility and stability delivery.

## Goal and evidence

Reduce model round trips and page content added to context when locating a fact,
while preserving RabbitHole's small tool catalogue. The owner supplied these
RH-AGENT-001 Wikipedia results from two rounds on 12/14 November 2026, using
`claude-sonnet-5-5` and medians of ten runs per arm:

| Metric | RabbitHole MCP | Chrome + Playwright MCP 0.0.85 |
| --- | ---: | ---: |
| Total tokens per task | 36,824 | 47,431 |
| MCP calls | 6–7 | 5 |
| Fixed context per model call | 4,220 tokens | 8,047 tokens |
| Context added during the task | 4,600–6,300 tokens | 3,200–3,500 tokens |
| Task cost, with prompt caching | USD 0.0313 | USD 0.0287 |

These are supplied historical measurements, not a benchmark rerun by the
development agent. The indicative targets are a median of at most four MCP
calls, 30,000 total tokens and 3,000 added tokens. They are estimates until
remeasured. The expected benefit comes primarily from avoiding `open` → `find`
→ `read` round trips and reading the article from its beginning unnecessarily.

## High priority

1. **Context around text matches in `find`.** Add integer `context`, default
   240 characters in total. Show a window centred on the first match, with `…`
   at clipped ends, or the whole accessible name when it fits. `context=0`
   preserves the existing first-120-character clipping. Keep case-insensitive
   matching and map match positions back to original characters: lowercasing
   can change length, so byte offsets in the lowercased string are unsafe.
   Report additional occurrences as `(+N más en este elemento)`. Preserve the
   core line grammar `@eN role "snippet" (position)` and existing `esc()` rules.
   A fixture with more than 500 characters and a match at character 400 must
   return that match without a subsequent `read`.
2. **Multiple search terms.** Let `text` accept a string or an array, matching
   any term. Return elements in document order; `limit` counts elements. Each
   result identifies the matching terms and centres its snippet on the first
   occurrence. List terms absent from the document at the end, for example
   `Sin coincidencias: "diciembre de 1989"`. A result limit must not turn a term
   present later in the document into a false absence. Test one-call results
   for `find {"text":["Guido","1989","CWI"]}` and an absent term. ECMAScript
   `regex` using our own engine is an optional later phase.
3. **Search during `open`.** Add `find` accepting the same string or array.
   Return the usual `[page] "title" URL status=…` header, warnings for covering
   layers such as `⚠ [dialog] …`, then search results. Omit landmarks, outline
   and forms in this mode unless `summary=true` requests both. Preserve normal
   navigation errors. A fixture's `open {"url":"…","find":["December 1989"]}`
   must return the relevant passage in one call and fewer than 600 characters.

## Medium priority

4. **Lighter `read`.** Add `links`, keeping **`true` by default** for existing
   navigation workflows. `links=false` emits plain anchor text, without refs.
   Omit citation links inside `<sup>` that target a fragment on the same page,
   unless `cites=true`; preserve other superscripts and external links. Collapse
   repeated whitespace, including nonbreaking spaces, and remove whitespace
   before `, . ; : )`, preserving meaningful preformatted content. The long
   paragraph fixture with `links=false` must be below 800 characters and contain
   no `@e`; default output must contain neither citation marks nor `" ,"`.
5. **Article summaries.** Allocate outline slots to the two highest heading
   levels first, preserving document order within that selection; use remaining
   slots for deeper headings. Show forms with a text field or submit button;
   count omitted preference/radio-only forms, e.g.
   `… +2 formularios sin campos de texto`. Keep `main`, `search` and dialogs
   with refs; group navigation landmarks as `nav ×5`, retaining the first ref.
   The article fixture summary must be below 900 characters and include the
   level-two sections that fit the heading limit.
6. **Compact tool guidance.** Describe `find` as multiple-term search with
   surrounding context. Explain that `find` is cheaper for a particular fact
   and that `read` without `ref` starts at the article beginning. Update the
   summary hint to mention `open … find=` and `find text=`. Keep the serialized
   `tools/list` JSON at **at most 6,900 characters**; record its exact serialization
   and actual current size before and after changes. Historical supplied sizes
   were 6,252 characters for RabbitHole and 21,177 for Playwright.

## Compatibility and validation

- Keep the short summary as the default `open` response, the `⟦page …⟧`
  untrusted-content fences, stable `@eN` refs, `snapshot` format, and existing
  `read` defaults for `max_tokens` and `offset`.
- Revalidate entry points in `crates/agent/src/{tools,read,summary,browser}.rs`.
  Add regression coverage in `crates/agent/tests/agent.rs` with an article
  fixture containing long text, links, same-page superscript citations,
  several heading levels and a radio-only form. Cover Unicode length-changing
  lowercase, clipping/escaping, limits and missing-term reporting as well.
- Update [the interface specification](AI_INTERFACE.md): tool table, search
  output grammar and summary behaviour. Document implemented behaviour only
  after code and tests pass.
- Repeat RH-AGENT-001 with **ten runs per arm**, recording raw executions,
  model/tool versions, prompts, cache conditions, calls, tokens, output sizes
  and cost. Five runs are insufficient for its observed four/five-call modes;
  historical estimated savings varied from 23.0% to 2.8% between rounds.
- Add a separate interaction benchmark before generalising to clicks, forms,
  JavaScript-heavy sites or cookie banners. The original evidence covers one
  factual Wikipedia task only.
