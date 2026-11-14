# Unicode tables

The JavaScript engine implements normalization itself over UTF-16. Its generated
normalization data uses Unicode 17.0.0, matching the existing RegExp tables;
it does not depend on the host Python Unicode database or a runtime library.
The Unicode data notice is in [LICENSE-UNICODE.txt](LICENSE-UNICODE.txt).

`gen_normalization_tables.py` consumes `UnicodeData.txt` and
`DerivedNormalizationProps.txt` from the official [Unicode 17 UCD](https://www.unicode.org/Public/17.0.0/ucd/).
It checks both complete input hashes before producing decomposition mappings,
nonzero combining classes and non-excluded composition pairs. Hangul is handled
algorithmically. Regenerate with:

```sh
python3 tools/unicode/gen_normalization_tables.py /path/to/ucd-17.0.0 > crates/js/src/builtins/string/normalization_data.rs
```

The library regressions cover Hangul, composition exclusions, equal-class
blocking, canonical ordering, compatibility, supplementary characters, isolated
surrogates and observable receiver/form coercion. A separate ignored test checks
all 20,034 rows and 400,680 identities from the official `NormalizationTest.txt`.
That external corpus is not bundled and the test is not part of the default
library run. Run it explicitly:

```sh
NORMALIZATION_TEST_FILE=/path/to/ucd-17.0.0/NormalizationTest.txt cargo test --release -p rabbithole-js --features jit unicode_normalization_conformance -- --ignored --nocapture
```

The expected Unicode 17 test corpus SHA-256 is
`5019ffd530751a741900c849c0e010332f142a3612234639bd200b82138a87db`.
The algorithm follows [Unicode Normalization Forms (UAX #15)](https://www.unicode.org/reports/tr15/),
using canonical decompositions plus `Full_Composition_Exclusion`; compatibility
forms additionally apply tagged decompositions. Isolated UTF-16 surrogates are
preserved and form reordering boundaries.
