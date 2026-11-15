# Option selectedness and dirtiness

The `selected` content attribute and `defaultSelected` describe the default.
The DOM stores live selectedness and dirtiness separately, so script/native
selection does not rewrite markup or lose the state when a change handler
adds/removes the chosen option's default attribute. The state belongs to the
Document, not a JS wrapper, and is copied when an option is cloned/imported.

`option.selected` marks that option dirty; `select.value` and `selectedIndex`
deselect all and mark only the matching option dirty. No match deliberately
leaves selection empty. `form.reset()` restores the selected attributes,
clears dirtiness and runs the single-selection fallback. Insertion/removal,
`multiple`, `size`, disabled-state changes and nondirty selected-attribute
changes also apply the select rules. A single drop-down initially chooses the
first enabled option; explicit empty selection is not invented by getters.

Script values, freshly requested `selectedOptions`, FormData, native form
submission, native option menus, CSS `:checked`/validity, layout, AX snapshots
and form descriptions use the live state. CSS `:default` and `[selected]`
continue to inspect the attribute. Existing collection wrappers remain
snapshot collections; making all HTMLCollections live is a separate concern.
This does not implement customizable select/selectedcontent, picker UI,
complete constraint validation or new form-ownership rules.

Normative references: HTML [select](https://html.spec.whatwg.org/multipage/form-elements.html#the-select-element)
and [option](https://html.spec.whatwg.org/multipage/form-elements.html#the-option-element)
algorithms, inspected 2026-11-15.

Targeted regression commands (not executed while preparing this candidate):

```sh
cargo test --profile ci --locked -p rabbithole-dom --test select_selectedness
cargo test --profile ci --features jit --locked -p rabbithole-engine --test select_selectedness
cargo test --profile ci --features jit --locked -p rabbithole-agent --test fill fill_select_preserves_dirty_selection_through_onchange_default_edits_and_reset
```

Run the Script/Engine/Agent targets with `RABBITHOLE_JIT=off RH_GC_STRESS=inc`
and `RABBITHOLE_JIT=on RABBITHOLE_JIT_THRESHOLD=0 RH_GC_STRESS=inc`.
Existing Script form-control tests and the complete `fill` target provide
additional regression coverage. These flags alone do not prove native entry.
