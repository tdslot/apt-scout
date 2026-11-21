# Localization

- `LOC-1` (planned) A locale MUST select the words a run prints and nothing else.
  It is never an input to a path, a filename, a decision, a comparison, an ordering, the plan, the
  bytes written or an exit code, and the privileged path never reads it: the locale is resolved once
  at startup, before any apt file is opened, and no module reached from `src/apply.rs` reads an
  environment variable. `LC_ALL` and `LANG` choose the words a run prints.
  With `NO_COLOR`, which only turns colour off, and `COLORTERM`, which only chooses which of the
  accent's two escapes is printed (Output), they are the only environment reads that can influence
  printed text: no other read can change a character a run prints, and none of the three reaches a
  decision, a path, a byte written or an exit code. `TMPDIR` is the only other environment variable
  the shipped binary reads — through `std::env::temp_dir()` in `src/verify.rs`, for the verifier's
  private 0700 directory — and it selects where that directory lives and nothing else: it reaches no
  decision, no `/etc/apt` path and no byte written, and a value that names no usable directory makes
  verification unavailable and a write fail closed.
  Two runs that differ only in locale print different text, write identical bytes and exit with the
  same code.
- `LOC-2` (planned) English MUST be the default and the fallback, and every user-facing message MUST
  have an English text.
  A run whose locale the binary does not carry prints English; a message the selected catalog lacks
  prints its whole English text, never a blank and never a half-translated line.
  The unit of fallback is the message, not the fragment.
- `LOC-3` (planned) Message ids MUST be stable and MUST never be keyed on English text.
  A message is addressed by a table key naming its role — `rejected_stale`,
  `table_header_signature` — matching `^[a-z][a-z0-9_]*$`: lowercase ASCII letters and digits joined
  by `_`, starting with a letter.
  The pattern is written anchored, and that is part of the requirement rather than notation: an
  unanchored `[a-z][a-z0-9_]*` matches an id-shaped run anywhere in a longer name and accepts
  `rejected.stale`, so the test states the anchored literal and asserts that the dotted name is
  rejected.
  The id space is the catalog format's, not the crate's module layout: a Fluent message id is
  `[a-zA-Z][a-zA-Z0-9_-]*` and a `.` there is attribute access, never part of an id, so a dotted
  name such as `rejected.stale` is not a message id any loader accepts, and the pattern above is a
  deliberate strict subset of the ids it does.
  An id never changes when its English sentence is rewritten: no id contains a space, so no id can
  be an English sentence, and editing English invalidates no translation.
- `LOC-4` (planned) Selection MUST be RFC 4647 lookup: `LC_ALL` when set and non-empty, else `LANG`,
  else English.
  No other environment variable, no option and no configuration file selects a locale: `LANGUAGE`
  and `LC_MESSAGES` are deliberately not read.
  The value is normalized to a language tag first — charset and modifier dropped, `_` written as
  `-`, `lt_LT.UTF-8` becomes `lt-LT` — and then matched against the shipped tags by lookup: the
  exact tag, else the language alone, else English.
  So `lt_LT.UTF-8` selects the `lt` catalog, and `de_AT.UTF-8` and `de_CH.UTF-8` both select `de`.
  A value that is not a language tag (`C`, `POSIX`, empty, unset) selects English without an error,
  and the program prints UTF-8 whatever the charset suffix claims.
- `LOC-5` (planned) Chinese MUST be selected by script, before the lookup, and a script MUST never be
  substituted.
  `zh_CN`, `zh_SG`, `zh_Hans_*` and bare `zh` select Simplified (`zh-Hans`); `zh_TW`, `zh_HK`,
  `zh_MO` and `zh_Hant_*` select Traditional (`zh-Hant`), which v1 does not carry, so those runs
  print English rather than Simplified.
  Script subtags are the only place the region decides the tag, because a plain lookup cannot tell
  Simplified from Traditional.
  The same rule applies everywhere: a locale the binary does not carry, including Norwegian Nynorsk
  (`nn`), prints English.
- `LOC-6` (planned) The catalogs MUST be Fluent files compiled into the binary.
  Each locale is one `locales/<tag>/main.ftl`, and `locales/en/main.ftl` is the source of truth every
  other catalog is translated from. `src/l10n.rs` builds them with `fluent-templates`' `static_loader!` and English
  as the fallback language, so the catalogs are compiled at build time: nothing is read from the
  filesystem at run time, no locale path comes from the environment, the release is still one static
  musl binary, and the shipped set is the set of directories under `locales/`.
- `LOC-7` (planned) The dependency set MUST change once, by decision, and `cargo deny check` MUST be
  what approves it.
  `fluent-templates` 0.15.1 and the crates it resolves to — `fluent-bundle`, `fluent-syntax`,
  `fluent-langneg`, `unic-langid`, `intl-memoizer`, `intl_pluralrules`, `self_cell`, `rustc-hash`,
  `type-map`, `smallvec`, `displaydoc`, `thiserror`, and the proc-macro crates
  `fluent-template-macros`, `syn`, `quote`, `proc-macro2`, `proc-macro-hack` and `unicode-ident`
  that build the loader and never link into the binary — are MIT OR Apache-2.0, a disjunction
  containing one of them, MIT alone, `Unlicense OR MIT`, `Apache-2.0 OR GPL-2.0-only`, `Unicode-3.0`
  for the ICU `tinystr`/`zerovec`/`zerofrom` crates, or `(MIT OR Apache-2.0) AND Unicode-3.0` for
  `unicode-ident`. `deny.toml`'s allow-list already covers every licence in that set and does not
  change; `cargo deny check` MUST stay green in the PR that adds the dependency, and that PR MUST list
  the resolved set with its licences.
  This is the crate's second dependency family and it is taken deliberately: one dependency is the
  default, not a prohibition, and a dependency enters by an explicit licence-checked decision rather
  than by accident.
- `LOC-8` (planned) A catalog MUST be complete or it MUST NOT be shipped.
  For every shipped locale the test suite MUST require every English message id, the same placeholder
  set in every message as its English entry, and every plural category that language's CLDR rule can
  return; a gap MUST fail `cargo test` with the list
  of missing ids, not the run, and that list is the work queue a translator starts from; the gate's
  own report is tested too, against a fixture catalog with one known id removed, so the failure text
  a translator works from is checked rather than assumed.
  Every message also carries a `#` comment above it naming when it is printed and what each
  placeholder holds, and a message without one MUST fail `cargo test`: the comment is what a translator
  reads, so a bare placeholder whose meaning is nowhere written is a defect.
  Translated text lives only in the catalog files: a translated string never appears in `src/`, in
  the documentation or in a test.
- `LOC-9` (planned) A catalog that does not parse MUST NOT ship.
  The tests MUST parse every `locales/*/main.ftl` and fail with the file and position of the first
  error, and `cargo test` also forces the compiled catalogs to load, so a catalog the binary carries
  and cannot parse is a test failure.
  A missing message id in a release binary prints a visible lookup diagnostic rather than nothing,
  and no lookup can change a decision.
- `LOC-10` (planned) A counted message MUST be one Fluent message with a select expression over the
  counted variable, and the plural category MUST be chosen by the library from the catalog's own
  language; the crate MUST carry no plural rule of its own.
  Numbers, byte counts, dates and durations are formatted identically in every locale, with ASCII
  digits and ISO 8601 dates; only the surrounding words are translated.
- `LOC-11` (planned) The run MUST NOT translate mirror hostnames, URIs, package names, file paths,
  option names, country codes, AS numbers, the program name and the `sudo apt-scout ...` line a
  non-root run prints: a
  user copies that line, so it stays runnable.
  Prompt answers (`y`, `yes`, `n`, `no`, and empty for the default) MUST NOT be translated either; only
  the question is.
  A printed message carries no character the user cannot see: Fluent's Unicode isolating marks
  around placeholders are disabled, so a copied URI is the URI.
- `LOC-12` (planned) Column alignment MUST count display columns, not characters, so a double-width
  script lines up in the ranked table like a Latin one.
  The width function is the crate's own, tested against a fixture table of code points whose East
  Asian Width is known, and the ranked table is padded with it rather than with a character count.
  Nothing is claimed for emoji, for combining sequences outside the listed ranges, or for
  right-to-left text.
- `LOC-13` (planned) Ordering MUST never use translated text.
  Where a run sorts, the key is a mirror URL, a message id or another value that is identical in
  every locale.
  A rejection carries its message id and its arguments and is rendered when it is printed, so no
  order can depend on the locale a run happens to print in.
- `LOC-14` Every release MUST carry the English manual page, and it MAY carry a page for any locale
  whose catalog it ships.
  A locale that carries a catalog MUST have its own page at `man/<locale>/man1/apt-scout.1`,
  installed
  to `/usr/share/man/<locale>/man1/`, where `<locale>` is the system name man-db resolves: `lt`, and
  `zh_CN` for the `zh-Hans` catalog, because man-db looks up a locale name and not a script tag.
  The page follows its catalog instead of leading it, so no release owes a page for a locale it does
  not carry: `ROADMAP.md` D33 delivers the first two, `lt` and `zh_CN`, after D31a and D32, and
  every locale added later ships its page in the PR that adds its catalog, under the same check.
  A page documents the same behavior and is checked with `mandoc -Tlint` and
  `man -L <locale> apt-scout` in a container.
  Its examples match that locale's `--help`, whose text comes from the catalog; `--help` is never a
  second document that can drift from it.
