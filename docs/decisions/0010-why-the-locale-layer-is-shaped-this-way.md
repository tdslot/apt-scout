# Why the locale layer is shaped this way

The whole localization chapter is planned work, and eight of its decisions are reasons rather than rules: what a
message id may be, how a locale is chosen, why the catalogs are compiled in, and why the manual page follows its
catalog. They are recorded here so the chapter can state the rules alone.

## A message id names a role, never English text

An id like `rejected_stale` describes when a message is printed. Keying on the English sentence would mean that
rewriting a sentence invalidates every translation of it, and that a translator's file has to track prose in another
language; an id that carries no space cannot be a sentence, so editing English invalidates nothing.

## The id pattern is anchored because an unanchored one accepts nonsense

`[a-z][a-z0-9_]*` unanchored matches an id-shaped run inside a longer name, so it accepts `rejected.stale`. The
anchored literal is therefore part of the rule and not notation, and the test asserts that the dotted name is
rejected. The pattern is also a strict subset of what Fluent accepts (`[a-zA-Z][a-zA-Z0-9_-]*`), because a `.` in
Fluent is attribute access and never part of an id.

## Selection is RFC 4647 lookup over a normalised tag

`LC_ALL`, else `LANG`, else English — no option, no configuration file, and deliberately not `LANGUAGE` or
`LC_MESSAGES`, so there is exactly one answer to "which catalog did this run use". The value is normalised first
(charset and modifier dropped, `_` written as `-`) and then matched by lookup: the exact tag, then the language,
then English. `lt_LT.UTF-8` therefore selects `lt`, and both `de_AT.UTF-8` and `de_CH.UTF-8` select `de`.

## Chinese is chosen by script, before the lookup

A plain lookup cannot tell Simplified from Traditional, so the region decides the script and the script decides the
catalog: `zh_CN`, `zh_SG`, `zh_Hans_*` and bare `zh` are Simplified, `zh_TW`, `zh_HK`, `zh_MO` and `zh_Hant_*` are
Traditional. v1 carries Simplified only, so a Traditional locale prints English rather than the wrong script.

## The catalogs are compiled into the binary

`fluent-templates`' static loader builds them at build time with English as the fallback language. Nothing is read
from the filesystem at run time, no locale path can come from the environment, the release stays one static musl
binary, and the shipped set is exactly the directories under `locales/`.

## The dependency set changes once, by decision

`fluent-templates` is the crate's second dependency family, and it enters by an explicit licence-checked decision
rather than by accident: one dependency is the default, not a prohibition. The resolved set is MIT OR Apache-2.0,
a disjunction containing one of those, MIT alone, `Unlicense OR MIT`, `Apache-2.0 OR GPL-2.0-only`, `Unicode-3.0`
for the ICU crates, or `(MIT OR Apache-2.0) AND Unicode-3.0` for `unicode-ident`; `deny.toml`'s allow-list already
covers every one of them and does not change, and `cargo deny check` is what approves the change.

The full resolution is 50 crates, measured in a scratch crate on 2026-11-13 and recorded in
`docs/history/m5.md` under D30a. The set the requirement names is the direct dependency and the
proc-macro crates it builds with: `fluent-templates` 0.15.1, `fluent-bundle`, `fluent-syntax`,
`fluent-langneg`, `unic-langid`, `intl-memoizer`, `intl_pluralrules`, `self_cell`,
`rustc-hash`, `type-map`, `smallvec`, `displaydoc`, `thiserror`,
`fluent-template-macros`, `syn`, `quote`, `proc-macro2`, `proc-macro-hack` and
`unicode-ident`. The rest of the 50 is the library's own tail, and three of those crates are not
build-time: `ignore`, `flume` and `log` are the `ignore` feature, which the library uses in its
run-time directory walk, and the `macros`-only build does not compile at 0.15.1 for that reason.

## Every message carries a comment because the catalog is a translator's only briefing

A message is a key and placeholders; what a placeholder holds is not visible in the string. The comment above
each message names when it is printed and what each placeholder carries, so a translator works from the file
rather than from a conversation, and a bare placeholder whose meaning is written nowhere is a defect the test
suite refuses. The English catalog is the source of truth the others are translated from, so a comment missing
there is missing everywhere.

## The locale never reaches a decision, a path or a byte

Three environment reads can influence printed text — the locale, `NO_COLOR` and `COLORTERM` — and one more,
`TMPDIR`, selects where the verifier's private directory lives. None of them reaches a path under `/etc/apt`, a
decision, an ordering, a byte written or an exit code. `docs/threat-model.md` rules on that inheritance as A2, which
is why the chapter cites it instead of repeating the argument.

## The manual page follows its catalog

A locale gets its page in the pull request that adds its catalog, so no release owes a page for a locale it does not
carry. The installed name is the one man-db resolves — `lt`, and `zh_CN` for the `zh-Hans` catalog — because man-db
looks up a locale name and not a script tag.

**Spec**: `docs/spec/localization.md`, `LOC-1` (the boundary), `LOC-3` (the id pattern), `LOC-4` and `LOC-5`
  (selection), `LOC-6` (compiled catalogs), `LOC-7` (the dependency), `LOC-8` (the comment) and `LOC-14` (the
  page). **Aliases**: D30a, D31a, D32, D33.