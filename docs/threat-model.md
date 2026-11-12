# D6e — Privilege boundary threat model and privilege-escalation decision

Status: **rulings decided; D6e NOT closed.** The nine rulings in section 7 are decided and human-approved, the three design gaps the first review left open are ruled in section 8, and the `SPEC.md` deltas they require are applied except where Appendix A marks an item `PENDING`. D6e stays open only until the independent security review of this record returns PASS. This is the privilege-boundary decision record for apt-scout: the threat model, the self-reexec ruling, the trusted root handoff, the tests that gate D9, and the state of every `SPEC.md` delta the rulings require.
Base: `origin/develop` with `8bbfa72` (D6f, PR #29) merged into `docs/d6e-threat-model`; the `path:line` citations were re-checked at `8fd9d70`, the D6f ones at `8bbfa72`, and citations elsewhere may have aged since. Roadmap item: D6e (blocks D9, D10, D11, D12, and every later write).
Authority: `ROADMAP.md` D6e/D6f/D9–D12, `SPEC.md` "Privilege boundary and threat model", "Apply", "Pipeline", "Testing", "TUI", and `AGENTS.md:108` (`Privilege-security gates`).

This document records four things: what may cross into a privileged phase, that the program never obtains privilege for itself, which tests must exist before D9 can merge, and what a write may trust about an `InRelease`. Section 7 is the decision record, section 8 records the three gaps the first review left open together with the ruling that closed each, and Appendix A states whether each `SPEC.md` delta is applied and, where it is not, who owns it. A superseded ruling is replaced in place, never annotated.

---

## 0. Reading basis and current state

Read for this document: `AGENTS.md` (whole file; `:108` `Privilege-security gates`), `ROADMAP.md` (whole file), `SPEC.md` (whole file, 192 lines), `GLOSSARY.md`, `src/verify.rs` (1–350), `src/http.rs` (1–250), `src/probe.rs` (1–180), `src/fresh.rs` (78–120), `src/sources.rs` (1–60, 380–520, 795–815), `src/main.rs` (1–120, 215–340, 485–585), `src/mirrors.rs` (245–300), `Cargo.toml`.

Facts that constrain the handoff. Each is the current code at this base, not a plan:

1. **There is no write path.** No `fs::write`, `File::create`, `rename`, or `set_permissions` exists in any non-test code path. The only files the program creates are the verification temp directory and temp file (`src/verify.rs:129–149`) and it removes both.
2. **No root check exists.** `geteuid`/`getuid` is not called anywhere; exit code 3 ("not root", `SPEC.md` Non-functional, exit codes) is unimplemented. `main` maps only the codes `run()` returns (`src/main.rs:572–580`; codes 1/2/4 at `src/main.rs:96–121`).
3. **`--apply`, `--restore`, `--yes`, `--no-tui`, `--refresh` do not exist.** `parse_args_from` accepts exactly `--list`, `--https-only`, `--no-geo`, `--country <CC>`, `--top <N>`, `-h/--help` (`src/main.rs:47–77`), and rejects anything else (`src/main.rs:75`). `--list` is accepted and ignored (`src/main.rs:44`). The whole run is read-only.
4. **No TUI exists.** `Cargo.toml` dependencies are `ureq` only; `ratatui`/`crossterm` arrive in D13. Terminal input is not an input today.
5. **The only environment variable the code consumes is `TMPDIR`,** indirectly, through `std::env::temp_dir()` in `private_temp_dir` (`src/verify.rs:54`). `HOME`, `PATH`, and `XDG_*` are never read. `std::env::args()` is the CLI source (`src/main.rs:31`).
6. **`InRelease` is verified with a fixed binary and keyring:** `/usr/bin/gpgv` (`src/verify.rs:15`), `/usr/share/keyrings/ubuntu-archive-keyring.gpg` (`src/verify.rs:16`), both `is_file`-checked before use (`src/verify.rs:101–127`), spawned with `env_clear()` plus `LC_ALL=C` (`src/verify.rs:156–166`), `--homedir` set to the private 0700 directory (`src/verify.rs:60–64`, `src/verify.rs:160`), stdin nulled (`src/verify.rs:165`), 10 s timeout (`src/verify.rs:20`), 1 MiB status cap (`src/verify.rs:21`), and `Verified` requires exit success plus `GOODSIG` plus `VALIDSIG` and no `BADSIG`/`ERRSIG`/`NO_PUBKEY` (`src/verify.rs:250–257`).
7. **The freshness `Date` has a strict and a tolerant parse.** `fresh::release_date` reads `Date:` only inside the cleartext-signed region and refuses any body whose preamble is not whitespace-only (`src/fresh.rs:103–129`); `fresh::release_date_tolerant` keeps the whole-body scan that display and ranking use (`src/fresh.rs:131–143`). The probe keeps both: `updates_date` for lag and the table, `signed_updates_date` only when the strict parse succeeded (`src/fresh.rs:155–190`).
8. **The write baseline is a value; the unforgeable plan is not built yet.** `Probed` carries `signature` and `signed_updates_date`, and `Probed::verified_updates()` yields `verify::VerifiedDate` only when both halves are present (`src/fresh.rs:165–181`); `Reference::verified_updates()` does the same (`src/probe.rs:88`); `write_baseline` takes the canonical reference when it is verified and otherwise the newest verified candidate, `None` when nothing verified (`src/main.rs:515–533`), so a write fails closed. Still D9's: `SignatureStatus::Verified` is a public variant any code in the crate can construct (`src/verify.rs:26`), and there is no verified-plan type.

## 1. Assets and what root would touch

| Asset | Why it matters | What the privileged phase would do | State today |
| --- | --- | --- | --- |
| `/etc/apt/sources.list` | Every `apt` invocation resolves and installs from these URIs; a hostile edit redirects all package traffic | Back up, then replace atomically with rewritten content | Read-only inventory (`src/sources.rs:409–421`); never written |
| `/etc/apt/sources.list.d/*.list`, `*.sources` | Same, per-repository; deb822 stanzas carry `Signed-By`, so replacing a URI moves trust to a different host under the same key | Back up, then replace atomically | Read-only inventory (`src/sources.rs:423–490`); never written |
| `/etc/apt/sources.list.d/system-security.sources` (Pop!_OS only) | Does not exist before the run; created by the rewrite | Create with exclusive create, record in the manifest | Planned content exists as `PlannedFile { create_new: true }` (`src/sources.rs:36–42`); never created |
| Backup directory `/var/backups/apt-scout/<UTC timestamp>/` | The only recovery path; if an attacker can write it, `--restore` becomes a root file-write primitive | Create 0700, write one copy per changed file, write the manifest, fsync | Does not exist; `SPEC.md` Apply defines it |
| Backup manifest inside that directory | Names the files `--restore` will overwrite; it is an *instruction to root* read back from disk | Written by root, later parsed by root | Undefined format; no code |
| `/var/log` output or the terminal | Root writes to a terminal that the unprivileged user may share; terminal escape sequences printed as root execute in the user's terminal context, not in root's | Print the diff, confirmation, and result | Output only via `println!` (`src/main.rs:311–340`); no TUI |
| The user's environment (`PATH`, `HOME`, `TMPDIR`, `XDG_*`, `http_proxy`) | A root process that resolves a command or a path through the environment executes attacker-chosen code | Must be cleared to a fixed minimum | Only `TMPDIR` is consumed (`src/verify.rs:54`); `PATH`/`HOME` never read |
| The user's own apt sources being *read* as root | Whatever root reads becomes what root writes back; a local user who can write a file under `/etc/apt` chooses its content | Re-read and compare against the plan | Read by name with `std::fs::read` (`src/sources.rs:492`) |
| `/var/cache`/`~/.cache/apt-scout/mirrors.json`, `/usr/bin/gpgv`, the keyring | Cache and verifier inputs decide ranking and verification | Root must not read or write any of them | Cache unimplemented; `gpgv` runs unprivileged today (`src/probe.rs:84`) |

## 2. Attack surface into the root phase

Every value below crosses into a process that will write as root. "Who controls it" is the threat model's answer to whether a control is required.

| # | Input | Entry point today | Who controls it | Crosses into root phase? |
| --- | --- | --- | --- | --- |
| A1 | CLI arguments | `src/main.rs:31`, parsed `src/main.rs:34–79` | The caller; under `sudo` also any local user who can influence the invoking shell | Yes — selects mode, path, and confirmation |
| A2 | Environment | `src/verify.rs:54` only | Any local user who can set the environment of the root process | Yes if `sudo` preserves it (no `env_reset` assumed) |
| A3 | apt source file contents | `src/sources.rs:382–520` | Package maintainers; a local user only if a file under `/etc/apt` is writable by them | Yes — becomes the bytes root writes back, minus rewritten URI tokens |
| A4 | Mirror URLs chosen by the program | `src/mirrors.rs:247`, filtered by `src/mirrors.rs:211` and `src/http.rs:68–92` | Launchpad HTML, `mirrors.ubuntu.com` text, and any host that can answer for them | Yes — the URL becomes the new apt URI |
| A5 | `InRelease` bytes | `src/probe.rs:78–86`, `src/probe.rs:120–131`, verified `src/verify.rs:87+` | The mirror (an untrusted host by definition) | Yes, as a verification result and a date |
| A6 | `InRelease` `Date` | `src/fresh.rs:103–143` (strict in-region parse, tolerant display parse), surfaced as `signed_updates_date` / `updates_date` (`src/fresh.rs:155–190`) | The mirror: a `Date` outside the signed region is display-only; one inside it is covered by the archive key | Yes — decides which mirror is written, and only the in-region value may be used for a write (3.9) |
| A7 | Backup manifest on disk | No code | Whoever can write inside the backup directory | Yes — drives `--restore` |
| A8 | File metadata and symlinks under `/etc/apt` | `src/sources.rs:383–466` (symlink refusals), owner/mode never read | A local user who can create entries in `/etc/apt/sources.list.d` (root-owned 0755 by default, so only root, unless the administrator relaxed it) | Yes — decides which paths root opens |
| A9 | The plan/selection from the unprivileged phase | Does not exist | The unprivileged phase; transitively A3–A6 | Yes — this is the handoff, section 5 |
| A10 | Terminal input | No TUI (`Cargo.toml`) | The user, and anything that can inject bytes into the TTY | Future (D13) — the confirm keystroke only |
| A11 | A hostile local user writing where the program reads | `TMPDIR` (`src/verify.rs:54`), pre-created `/var/backups/apt-scout`, pre-created `system-security.sources` | Any local user | Yes |

Notes that change what a control must do:

- A4 is not a "malicious mirror" only. `mirrors.ubuntu.com` is plain HTTP (`SPEC.md` Pipeline step 3.2), and candidate URLs from that source are marked unverified today. Any host that wins the ranking becomes an apt URI: `apt` will then fetch `Packages` and `.deb` files from it and verify them against `Signed-By`, so package content stays authenticated, but *which* server the machine trusts the availability of is chosen by ranking.
- A5/A6 do not authorize installation. Even a forged `Date` cannot install a package: `apt` still checks archive signatures. What A6 buys an attacker is the position of the *selected* mirror, which is then written to `/etc/apt`. That is the whole weight of D6f P1 #1 — see 3.9.
- A11 matters most where a directory is created on first use. The old `SPEC.md` Errors and self-healing rule created missing own directories with `create_dir_all`, which succeeds on a directory another user already created; the rule now covers the cache directory only, and the backup root fails closed instead (7.6).

## 3. Trust boundaries and controls

Each boundary lists what exists with `file:line`, then what is missing and what the code does instead. "Missing" means: if D9 were written today on top of this code, that input would reach a write unprotected.

### B1 — Arguments and environment → program (unprivileged)

Existing:
- Country is uppercased and shape-checked before any lookup (`src/main.rs:51–62`); unknown codes are rejected against the Launchpad country table before any network call (`src/main.rs:81–94`, wired at `src/main.rs:96`).
- Architecture and country are validated in preflight before geolocation or HTTP (`src/main.rs:95–107`).
- Unknown options are hard errors (`src/main.rs:75`).
- `gpgv` runs with `env_clear()` plus `LC_ALL=C` (`src/verify.rs:156–158`), so A2 cannot reach the verifier.

Missing:
- No `geteuid` check and no exit-3 path, so nothing today distinguishes a privileged run.
- Nothing validates `TMPDIR`: `private_temp_dir` joins it to a random name and creates that directory 0700, but the parent is never checked for owner, mode, or sticky bit, and a `TMPDIR` on a world-writable mount is accepted. `SPEC.md` Freshness trust boundary claims "a private 0700 temporary directory"; the directory is private, the parent is untrusted. Already recorded as a D6f P2.
- Nothing clears the environment for the *program*, only for the verifier. A privileged phase must build its own minimal environment rather than inherit one.

Required before D9: the privileged phase refuses to run on any uid other than 0 (exit 3 with the exact command to re-run), reads no environment variable, and passes a constructed environment to any child.

### B2 — apt source contents → inventory (unprivileged)

Existing:
- The root directory is refused unless `symlink_metadata` reports a directory (`src/sources.rs:383–405`).
- `/etc/apt/sources.list` is accepted only when `symlink_metadata` says regular file (`src/sources.rs:409–421`); the same for `sources.list.d` (`src/sources.rs:423–425`, refusal at `src/sources.rs:479–483`).
- Only `.list` and `.sources` are read; other names are reported and skipped (`src/sources.rs:444–456`).
- Directory entries are filtered on `entry.file_type()` (`src/sources.rs:458–466`), which does not follow symlinks.
- Non-UTF-8 files are reported and skipped, never guessed (`src/sources.rs:497–503`).

Missing:
- No ownership or mode check. A file owned by uid 1000 with mode 0666 under `/etc/apt/sources.list.d/` is inventoried and, in a later milestone, would be rewritten by root. The code reads whatever is there.
- No TOCTOU resistance: `symlink_metadata`/`file_type` checks at `src/sources.rs:410` and `src/sources.rs:458` are separate syscalls from the `std::fs::read(&path)` at `src/sources.rs:492`, which re-opens by name and therefore follows a symlink substituted in between. A local attacker who can replace the entry wins a root read of any root-readable file, whose content then lands in the plan and in `/etc/apt` (world-readable). Std-only fix without a new dependency: open with `OpenOptions`, then compare `File::metadata()?` `dev`/`ino` (`std::os::unix::fs::MetadataExt`) against the `symlink_metadata` result, and use the open handle for the read. `libc::O_NOFOLLOW` is not available without a new dependency.

Required before D9: the privileged phase opens every target with the identity check above, and aborts on any change.

### B3 — Network responses → candidates, dates, signatures (unprivileged)

Existing:
- One 10 s budget per call, not per hop (`src/http.rs:12`, `src/http.rs:98`, `src/http.rs:152–160`).
- Redirects followed by hand, every hop re-validated (`src/http.rs:99`, `src/http.rs:150`), at most 3 (`src/http.rs:17`), `https`→`http` downgrade refused (`src/http.rs:189–195`), protocol-relative and relative `Location` refused (`src/http.rs:128–142`).
- Proxies explicitly disabled so the environment cannot reroute traffic (`src/http.rs:101`).
- Private, loopback, and link-local addresses filtered at resolution, on the exact addresses connected to (`src/http.rs:68–92`, `src/mirrors.rs:211`).
- Size caps: 2 MiB pages, 256 KiB small files (`src/http.rs:14–16`), 512 KiB for `InRelease` (`src/probe.rs:13`, `src/verify.rs:19`), 4 KiB for the informational `-security` head (`src/probe.rs:15`), 1 MiB sample (`src/probe.rs:16`).
- `Verified` requires exit success, `GOODSIG`, `VALIDSIG`, and no `BADSIG`/`ERRSIG`/`NO_PUBKEY` (`src/verify.rs:250–257`).

Missing:
- A6: closed by D6f — the write path takes `signed_updates_date`, display keeps the tolerant parse (3.9).
- No verified-plan type: `VerifiedDate` exists (3.10), the plan-level gate is D9's (G2).
- The throughput sample is attacker-controlled: only the xz magic and a 64 KiB minimum are checked (`src/probe.rs:18`, `src/probe.rs:153–156`). A mirror that streams 1 MiB of zeros at line rate wins the speed tie-break and can win the group. Accepted risk with a stated reason: throughput cannot be verified from the outside, `SPEC.md` Pipeline step 9 already treats it as relative, and the loser of that lie is a slow mirror, not an unauthenticated package.
- The `-security` head is unverified by design (`src/probe.rs:89–97`, `SPEC.md` Freshness trust boundary). Nothing may consume it for a write; today it feeds only display.

### B4 — Selected mirror URL → rewrite plan

Existing:
- `rewrite_source` re-validates the selected URI before planning (`src/sources.rs:800–807`).
- `validate_url` enforces `http`/`https` only, no backslash, no `@`, no whitespace or control characters, ≤2048 bytes, valid port, bracketed IPv6 parsed, local names rejected, numeric private hosts rejected (`src/mirrors.rs:247–324`, `src/mirrors.rs:161–211`).

Missing:
- Nothing ties the selected URI back to the candidate set that was actually probed. A byte-identical plan can be produced from a URI that appeared in no mirror list, because `rewrite_source` validates shape only.
- Nothing requires the selected URI's `-updates` `InRelease` to have been `Verified` in the same run. `fresh::Probed` has no field for it (finding 8).
- Nothing restricts the URI host to a *public resolvable* host at plan time. Resolution happens in the prober (`src/http.rs:70–92`), so a candidate's DNS answer can differ between probe and write. This is bounded: the write phase does not connect to the host, it only writes the string.

Required before D9: the plan carries the URI, the digest of the candidate it came from, and a verification proof minted by the verification pass in the same process; the privileged phase accepts nothing else.

### B5 — Backup directory and manifest → root read-back and restore

Existing: nothing. There is no backup code, no manifest format, and no `--restore`.

What exists nearby: `SPEC.md` Apply requires mode 0700, refusal of a symlinked directory, a manifest of original paths, and aborting the whole apply on any backup failure; `SPEC.md` Apply requires `--restore` to validate that every manifest path is under `/etc/apt/` and a regular file after resolution.

Missing, and required before D9:
- The backup root must be verified after creation: `symlink_metadata` says directory, `uid == 0`, mode exactly 0700, and the path is not a symlink. A pre-created `/var/backups/apt-scout` by uid 1000 must be a hard abort, not a `create_dir_all` no-op. `SPEC.md` Errors and self-healing now states that exception (Appendix A item 6): the `create_dir_all` self-heal covers the cache directory only.
- The manifest must be written by root, mode 0600, and read back with the dev/ino identity check of B2.
- The manifest must not be able to name a path. It carries a target *identifier* from a closed enum plus a file name validated against `[A-Za-z0-9._-]{1,64}` with a `.list`/`.sources` suffix; the path is then built as `/etc/apt/sources.list` or `/etc/apt/sources.list.d/<name>`. Containment is by construction, not by string prefix checking.
- Each record must carry the original digest, mode, and owner so a restore can prove it is restoring what it backed up, and refuse otherwise.

### B6 — Plan or selection handed from the unprivileged phase to root

Existing: nothing. See section 5.

### B7 — Terminal input (TUI, D13+)

Existing: no TUI, no `ratatui` dependency (`Cargo.toml`), so the surface does not exist yet.

What the design imposes now so D13 does not have to revisit it:
- A keystroke is authorization for exactly one thing: the confirm step. It never carries data.
- Custom mirror URLs typed in the TUI (`SPEC.md` Apply, Custom URLs) are input A4: they are validated and probed in the unprivileged phase and reach the plan only through B4's re-validation.
- Nothing that root prints may contain unescaped mirror-controlled bytes. A mirror's URL, `Origin`, `Label`, or error text can contain `ESC`; printing it as root into a shared terminal is a control-sequence injection into the user's terminal. The privileged phase must render untrusted strings with control characters replaced. Nothing does this today (`src/main.rs:311–340` prints URLs raw) — accepted while the program is unprivileged and root never runs.

### B8 — A hostile local user writing where the program reads

Existing:
- The verification temp name is 8 random bytes plus the pid, created `create_new` and mode 0600, in a directory created 0700 (`src/verify.rs:49–66`, `src/verify.rs:129–149`), so a pre-created file or directory at that path causes a clean `Unavailable`.

Missing:
- The temp parent is `env::temp_dir()` (`src/verify.rs:54`) with no owner/mode/sticky check.
- Any future backup or plan directory created on first use is not verified after creation (B5).
- `system-security.sources` creation must be `create_new` (exclusive), never overwrite: the enum discriminator already exists (`src/sources.rs:39`), no code uses it.

### 3.9 D6f P1 #1 — a write may use only a `Date` from inside the signed region

What the code does: `fresh::release_date` accepts a body only when nothing but whitespace-only lines precedes a line that is exactly `-----BEGIN PGP SIGNED MESSAGE-----`, skips the armour headers to the first blank line, and reads `Date:` before `-----BEGIN PGP SIGNATURE-----` (`src/fresh.rs:103–129`). `fresh::release_date_tolerant` keeps the whole-body scan for display and ranking (`src/fresh.rs:131–143`), and the probe carries both values (`src/fresh.rs:155–190`). Why the rule exists: `gpgv` ignores everything before the armour, so a mirror can prepend `Date: <chosen>` to a genuinely signed body and the signature still verifies; the status stays `verified`, and only the parser separates a signed date from a displayed one.

Ruling: **for a write, the date must come from inside the cleartext-signed region, and the region must be located by the parser, not by the verifier.**

Required behavior, both sides:
- Only whitespace-only lines may precede `-----BEGIN PGP SIGNED MESSAGE-----`, and the armour line must start its own line. A body with any other prefix is **not** `Invalid`: `gpgv` verifies it and the status stays `verified`, because the prefix is outside what the signature covers, not something that breaks it. What such a body cannot supply is a trusted `Date`, so a write may not use it, while read-only ranking keeps displaying and ranking that date under the `SPEC.md` Freshness trust boundary sentence ("ranking may display and use an `InRelease` `Date` even when signature verification is `invalid` or `unavailable`").
- The `Date:` header is taken from the cleartext between the armour headers (after the `Hash:` line) and `-----BEGIN PGP SIGNATURE-----`. A `Date:` occurring after the signature block is not the release date.
- A write requires: parse succeeded inside the signed region *and* `SignatureStatus::Verified`. Both facts travel together (section 5), so no later code can combine one without the other.
- Regression fixture: `tests/fixtures/signed/noble-updates-InRelease.prefixed` — the existing fixture with a forged `Date:` prepended — must produce a date equal to the genuine one for a write, or an outright refusal; it must never produce the forged date. The hand proof in D6f (`gpgv` exits 0 with `GOODSIG`/`VALIDSIG` on the prefixed copy) is the reason this test is mandatory.

What this meant for a write, and why it blocked D9 rather than merely degrading read-only output: without it, every other control in this document inherited a forgeable input. D6f implemented it (PR #29); 7.4 records the decision.

### 3.10 D6f P1 #2 — the write baseline is a verified value

What the code does: `Probed` carries `signature` and `signed_updates_date`, and `Probed::verified_updates()` yields `verify::VerifiedDate` only when both are present (`src/fresh.rs:165–181`); the baseline is the canonical reference when `Reference::verified_updates()` is `Some`, otherwise the newest candidate whose own `verified_updates()` is `Some`, and absent when nothing verified, so the write fails closed (`src/main.rs:515–533`). Still missing and D9's to build: an unforgeable `Verified` (`SignatureStatus::Verified` is a public variant, `src/verify.rs:26`) and the verified-plan type.

Ruling: **"verified" must be a value, not a string and not a bool, and the write gate must be able to accept only that value.**

Required behavior:
- One constructor produces the proof: the verification pass itself, in the unprivileged phase, in the same run. `SignatureStatus::Verified` becomes unconstructible outside `verify` (private module constructor; `is_verified()` stays for display).
- The proof carries the parsed date and the fact that the date came from the signed region (3.9), so the two P1s close into a single value that cannot be assembled from parts.
- `fresh::Probed` carries the proof's status, and the baseline is `Verified(date)` or absent. `fallback_baseline` contributes a date only from candidates whose `-updates` `InRelease` was verified in this run. A forged future date is then impossible rather than merely implausible: a genuine Ubuntu signature cannot cover an attacker's `Date`.
- If no candidate verifies, the write fails closed with an explanation naming the cause (missing `gpgv`, missing keyring, every mirror `invalid`), and read-only output is unchanged.

What this meant for a write, and why it blocked D9: the accepted rule in `SPEC.md` Freshness trust boundary ("the selected mirror's `-updates` `InRelease` and the canonical reference used as freshness baseline must both be `Verified`") had no representation, so it could not be enforced by construction. D6f implemented the baseline half (PR #29); 7.5 records the decision, and the plan-level half is D9's under G2.

## 4. Self-reexec: the explicit decision

### 4.1 The constraint that decides it

A process cannot obtain privilege for itself, so "self-reexec" is shorthand for one of three mechanisms, and they are not equivalent:

1. exec itself under `sudo` — the program invokes `sudo`. Forbidden by `AGENTS.md:108` (`Privilege-security gates`: "Never add or assume `sudo` self-reexec is safe") and by `SPEC.md` Privilege boundary bullet 4, which states that the program never re-execs itself privileged and never invokes `sudo`. The ban stands in the ruled design: a program that execs `sudo` converts every path it controls into a root path, and inherits `sudo`'s environment policy as its only defense.
2. install a setuid-root bit — the binary becomes a local privilege-escalation candidate by definition. Any local user can run the write phase. The only thing standing between them and root file writes is the program's own validation of an input that the same local user supplies.
3. ask the user to run the second phase as root — the mechanism the user's own `sudo` provides, with no privilege manufactured by the program. This is the ruling (7.1).

Mechanism 3 is the ruling (7.1); mechanisms 1 and 2 are the rejected forms analysed in 4.3 and 4.4. The question D6e answered is what the second, root-only invocation is allowed to contain, and the answer is section 5: a plan, and nothing else.

### 4.2 The ruling — no self-reexec: refuse and print the exact command

The program never execs anything privileged. `--apply` as a non-root uid prints the exact two-invocation command and exits 3. The privileged invocation is a separate, narrow mode that reads a validated plan (section 5) and performs only file writes.

- **Buys:** the smallest privileged surface that satisfies `SPEC.md` Privilege boundary bullet 3 ("Root is limited to a small final operation over a previously validated plan"); no setuid bit, no internal `sudo`, no second executable to package; the privileged path is reachable only by an explicit human action with a visible command line; the unprivileged phase keeps every network request, `gpgv` invocation, and parse, which is what bullet 3 requires.
- **Costs:** two invocations, so the plan must be transportable (section 5) — which every option needs anyway once network access may not run as root; the user has to read and copy a command.
- **How it fails:** visibly. Wrong uid → exit 3 with the command, no file opened. Missing plan → exit 2. Plan invalid → exit 1 with the failing field, nothing written. No partial state is reachable, because the first thing the privileged mode does is validate the whole plan before opening any target.
- **What an attacker gains:** the same power the user has, because a process running as the invoking uid can invoke the privileged phase itself. What root adds is bounded: it writes only the bytes the plan's digests name, only to targets in the closed enumeration, and only while every digest it recomputes agrees with the plan and with the disk (5.4). That guarantee is content binding, not producer identity, and G2 records the limit plainly.
- **Testable in disposable containers:** fully. `docker run` as root exercises the privileged phase; the refusal case runs as uid 1000; the plan handoff is fed from a fixture over stdin. No setuid, no nested privilege boundary to simulate.

### 4.3 Rejected — self-reexec through a fixed absolute path, minimal environment, pre-write confirmation token

The program re-execs `/proc/self/exe` (absolute, not `PATH`-derived) with a cleared environment, passing the plan over an inherited fd and a token that the second phase requires before writing.

- **Buys:** one command for the user. A minimal environment makes A2 explicit rather than inherited. A token makes a stale or replayed invocation fail.
- **Costs:** the privilege still has to come from somewhere. Without `sudo` inside the program and without a setuid bit, the re-exec runs at the same uid as the parent, so nothing is gained; with the setuid bit, the audit surface is a setuid root binary that reads a plan from an unprivileged writer. That combination is the classic local-root pattern, and the token does not change it: a token the unprivileged caller generates is not a MAC, and if the root phase generates it, the caller never sees it. So Option B is either Option A with extra machinery or a setuid binary.
- **How it fails:** opaquely, which is the worse problem. A re-exec under an unexpected uid, a `/proc/self/exe` that reports `(deleted)` because the package was upgraded mid-run, an environment that `sudo` rewrote — each produces a failure in a process the user did not start and cannot inspect from the command line.
- **What an attacker gains:** in the setuid form, everything: a local user runs the write phase, and the plan they supply is validated only by code they can read and probe. In the non-setuid form, nothing beyond Option A, at the cost of a second code path.
- **Testable:** partly. The non-setuid paths are testable; the setuid path needs a container with a setuid install step, and the interesting failures (deleted exe, environment rewrite) are the ones that are hardest to reproduce deterministically.

### 4.4 Deferred — privileged helper process spawned once with a validated plan

The root-run process spawns one child (or ships one second binary) that owns every privileged operation and links nothing but `std`.

Two forms, and only one is compatible with the ruling:
- **C-spawned-by-root:** the user runs the write mode as root; it forks a child that reads the plan from a pipe and performs the writes. If the second binary is separate, it contains no `ureq`, no `gpgv` spawn, no TUI, no DNS — the privileged image is a few hundred lines and its interface is a byte schema. Out for v1: this is not an option held in reserve, and reintroducing it is a new product decision by the user (4.6, G3).
- **C-setuid-helper:** the unprivileged parent spawns a setuid helper. Same rejection as Option B: it manufactures privilege from an unprivileged caller.

- **Buys:** structural isolation that a reviewer can see in the dependency graph rather than having to trust a discipline. The interface between the two halves is enforced by the OS, and the privileged binary's "does not make network requests" claim becomes checkable (`cargo tree` on a two-crate workspace) instead of checkable-by-grep.
- **Costs:** a second binary in `.deb` assets (`Cargo.toml` `package.metadata.deb.assets`, currently one entry) and in the release tarball, a workspace split or a feature-gated second `[[bin]]`, and two artifacts to keep in version lockstep (the plan carries a format version for that reason). It also doubles the amount of packaging and release surface tested before D9.
- **How it fails:** like Option A, plus one new failure: a version skew between the two binaries. Mitigated by requiring an exact plan-format version match and refusing otherwise.
- **What an attacker gains:** nothing beyond Option A. The helper is reachable only from root.
- **Testable:** fully, in the same containers; the container test must additionally assert that the privileged binary contains no network-capable dependency, which is a stronger and cheaper claim to verify than a grep review.

### 4.5 Ruling and reason

**Option A. DECIDED 2026-11-12 by the user (via the parent orchestrator). The privileged phase is one binary, and a split into a separate privileged artifact is out of scope for v1 (4.6, G3).** Specifically: no self-reexec anywhere in the codebase; the unprivileged run computes and prints the plan and the exact `sudo` command; `--apply` on a non-root uid exits 3 without opening a file; the privileged invocation is a narrow mode that reads the plan from an inherited fd, validates it fully (section 5), verifies every on-disk digest, and writes — and never makes a network request, never runs `gpgv`, never invokes a shell, and never resolves a command through `PATH`.

Reasoning, in order of weight:
1. It is the only option that keeps `SPEC.md` Privilege boundary bullet 3 true without a setuid binary or an internal `sudo` call. Options B and C do not add privilege; they add code that pretends to.
2. The security property that matters is not "one process" but "the root process's input is a closed schema validated against disk state". That property is identical in A and C-spawned-by-root, so the extra artifact buys isolation of *code*, not of *authority* — and it costs packaging surface before D9, when the privileged code is still small.
3. Fewer concepts: one binary, one format, one refusal path, one test matrix. Option B's token and second code path are concepts that exist to solve a problem Option A does not have.
4. Testability: the refusal path, the plan validation, and the write path are all reachable from a container command line, with no setuid install step and no nested privilege transition to reason about.

### 4.6 One binary, and the triggers that remain

- **One binary — decided, not a threshold.** The program is not complicated, so it is one binary: the privileged phase is a mode of the same executable, and a separate privileged artifact is out of scope for v1. Nothing in this record reopens that — not a size measurement, not the shape of `src/apply.rs`, not a code review finding (G3). A future split of the privileged phase into a second artifact would be a new product decision taken by the user, with the threat model 4.4 describes; it is not something a threshold triggers and not a decision an agent may take, summarize or argue from a line count.
- If the user-facing cost turns out to dominate: a two-invocation flow that people get wrong (for example by running the whole command under `sudo`, so the network phase runs as root) is worse than a documented one-command flow. Mitigation inside the ruling: the refusal message prints the exact command *and* the root phase refuses a plan whose producer metadata says the plan was made by uid 0 (7.3).
- If a future distribution requirement forces a setuid or polkit integration (a GUI, or a non-terminal apply), the analysis in 4.3 and 4.4 applies unchanged: that path needs its own threat model and its own D6e-equivalent decision. It is out of scope for v1, which is a terminal tool.
- If plan transport over an inherited fd proves unreliable under the user's `sudo` configuration (`sudo` with a closed stdin, or `requiretty`), there is no second transport to fall back to: 7.2 admits an inherited descriptor as the only form, so the fix belongs in the handoff (for example keeping the descriptor open across the privilege transition), never in a plan file that root would have to open by path.

## 5. Trusted root handoff

### 5.1 Principle

The privileged phase may re-read `/etc/apt` (it must, to detect races and to verify digests) but may not re-derive decisions. Every decision — which mirror, which files, which bytes, which baseline, whether verification passed — is made in the unprivileged phase and travels as data, with the single exception G1 rules: a restore's source is root-only, so root reads it and the plan pins what it read. The privileged phase validates that data against a closed schema and against the current on-disk state, and refuses anything it cannot confirm. It has no fallback and no self-repair: a mismatch aborts with nothing written.

Why this and not "send the selection and let root recompute": recomputation would require the privileged phase to host the inventory parser and the rewrite engine, which (a) puts untrusted-input parsing in the root path, contradicting `SPEC.md` Privilege boundary bullet 3, and (b) means the bytes written are not the bytes the user confirmed in the diff (`SPEC.md` TUI, step 2, "old URI -> new URI"). Wrong on both counts.

`--restore` follows the same principle with a different reader of the source of truth. The backup directory is 0700 root-owned, the manifest 0600, so no unprivileged run can read them, plan from them, or know the manifest's digest; a restore plan therefore carries the backup id and the digest of the manifest it was built from, and is produced by a run that holds root. The privileged phase re-reads `/var/backups/apt-scout/<id>/MANIFEST` itself, compares the digest, and only then re-derives the target set and every per-target digest from the manifest, so the plan's file list is an assertion that must match exactly and never the source of truth for what gets written (5.4, G1). Both invocations of a restore are root. That does not weaken the apply model: a restore has no network phase and no third-party input, and its trust boundary is the backup directory's own 0700/0600, not a fetched body.

### 5.2 Shape

```rust
// An apply plan is written by the unprivileged phase; a restore plan by a root run that read the
// backup. Both are read and re-validated by the privileged phase.
struct WritePlan {
    format: u16,              // exact match required; a mismatch is a hard abort
    producer_uid: u32,        // Apply: refused when 0. Restore: refused when not 0 (7.3, G1)
    codename: String,         // == the codename read from os-release by the same run
    op: PlanOp,               // the discriminator: proof exists only inside Apply (G1)
    files: Vec<PlannedWrite>,
}

enum PlanOp {
    Apply {
        selected_uri: String,        // mirrors::validate_url shape + baseline proof below
        proof: WriteProof,           // section 3.9/3.10, minted by verify, not constructible elsewhere
    },
    Restore {
        backup: BackupId,            // a name, never a path: the root phase builds the path itself
        manifest_digest: [u8; 32],   // SHA-256 of <backup>/MANIFEST, re-read and compared by root
    },
}

struct BackupId(String);   // validated: ^[0-9]{8}T[0-9]{6}Z$; built into /var/backups/apt-scout/<id>/

struct WriteProof {
    mirror: SignedDate,              // the selected mirror's -updates InRelease
    baseline: BaselineKind,          // Canonical(SignedDate) | NewestVerified(Vec<SignedDate>)
}

struct SignedDate {
    date: i64,                 // parsed inside the cleartext-signed region only
    // verification result and signed-region flag are private fields of this type
}

struct PlannedWrite {
    target: Target,            // closed enum: no arbitrary paths exist in the format
    original_digest: [u8; 32], // SHA-256 of the bytes this plan was computed from
    original_mode: u32,
    original_uid: u32,
    original_gid: u32,
    content: Vec<u8>,          // Apply: the exact new bytes, byte-for-byte as shown in the diff.
                               // Restore: empty; the bytes come from the backup after it is validated
    content_digest: [u8; 32],  // SHA-256 of the bytes this plan will write (Apply) or restore (Restore)
    create_new: bool,          // accepted only for SourcesListD("system-security.sources") on Pop!_OS (5.4, 7.8)
}

enum Target {
    SourcesList,                        // /etc/apt/sources.list
    SourcesListD(FileName),             // /etc/apt/sources.list.d/<name>
}

struct FileName(String);               // validated: ^[A-Za-z0-9._-]{1,64}$ and .list|.sources
```

`PlanOp` is the discriminator (G1): a proof exists only inside `Apply` and a manifest digest only inside `Restore`, so a plan cannot drop the proof by claiming to be a restore and a restore plan cannot carry one. `BackupId` is a name validated as a UTC timestamp, so the privileged phase builds the manifest path from a fixed prefix plus a name that cannot contain `/` or `..`.

No path strings cross the boundary. `Target` is closed, and `FileName` cannot contain `/`, `.` alone, or `..`, so containment under `/etc/apt` holds by construction; a reviewer does not have to check a prefix comparison. `alternatives` need no separate field: they are `# alternative:` comment lines inside `content`, already rendered.

### 5.3 Encoding and transport

Encoding: a length-prefixed container — magic `APTSCPLAN`, format `u16`, then tagged records (`u16 tag`, `u32 length`, bytes). Reasons: `content` is arbitrary bytes and needs no escaping; a text format would need an escaping rule, and an escaping bug in the privileged parser is exactly the class of bug this handoff must not have. The parser is pure, lives outside the privileged code path, is unit-tested with hostile inputs (unknown tag, duplicate tag, oversize field, truncated record, non-UTF-8 where UTF-8 is required), and rejects everything it does not explicitly accept.

Transport: the plan arrives on an inherited file descriptor (stdin), and no plan-path argument exists at all (7.2). A path argument is A11: the privileged phase would read a file from a directory the invoking user can write. With stdin, the user's `sudo` command line carries no writable path:

```
apt-scout --apply --plan-out - | sudo apt-scout --plan-in -
```

The plan is at most a few hundred KiB on a typical system — one copy of each changed source file plus small fixed fields — and the reader enforces a 1 MiB limit and aborts beyond it. `--plan-in` accepts `-` and nothing else, so there is no plan-file form to guard: the privileged phase never opens a path to obtain a plan, and no directory has to be trusted for one.

### 5.4 Who validates what

| Field | Unprivileged phase | Privileged phase |
| --- | --- | --- |
| `format` | writes the current version | equal to its own; else abort |
| `producer_uid` | writes its own effective uid | for an `Apply` plan it must not be 0, and for a `Restore` plan it must be 0: a plan from a root-run producer is refused when it claims to be an apply, and a plan from a non-root producer is refused when it claims to be a restore, because no such run can have read the backup (7.3, G1). Both directions fail closed. This field is honest-producer metadata, not an anti-tamper control: a process running as the invoking uid can invoke the privileged phase itself, so what the digests buy is content binding, never producer identity (G2) |
| `op` | writes `Apply` or `Restore` from the mode it ran in | the discriminator decides which fields must exist while the plan is parsed, before any field is used: an `Apply` plan without a proof does not parse, and a `Restore` plan carrying a proof or a uri does not parse (5.2, G1) |
| `codename` | from os-release, used to build URLs | equals the codename read from `/etc/os-release` in this run; else abort |
| `selected_uri` | `mirrors::validate_url`, came from the ranked candidate set | `mirrors::validate_url` again (never trust a shape), and it must match the `content` it appears in |
| `proof` | minted by the verification pass: signed-region date + `Verified` | present, internally consistent, and its date matches the baseline logic that ranked it; a plan without a proof is refused, not downgraded. `Apply` only: the discriminator has no field for one in a `Restore` (G1) |
| `manifest_digest` | `Restore` only: digests the manifest it read | reads `<backup>/MANIFEST` by a path it builds itself, hashes it, and aborts on any difference **before** parsing it; only then does it re-derive the target set and every per-target digest from the manifest, so the plan's file list is an assertion that must match and never the source of truth for what is written (G1) |
| `target` | derived from the inventory | enum re-checked; the name re-validated; the resulting path `symlink_metadata`-checked as a regular file, uid 0, and the *directory* `/etc/apt/sources.list.d` checked for root ownership and no group/other write |
| `original_digest`/`mode`/`uid`/`gid` | reads them at plan time | re-reads at open time on the opened handle and compares; any difference aborts before any write |
| `content_digest` | digests the bytes it will write (Apply) or the backup copy it will restore (Restore) | recomputes it and aborts on any difference; no byte reaches a target without a digest that agrees (G2) |
| `content` | produced by `sources::rewrite_source` (pure) | used verbatim; no re-render. `Apply` only: a `Restore` plan carries no bytes, and the restored bytes come from the backup once the manifest digest matches |
| `create_new` | true only for the Pop!_OS security split file | accepted **only** when the platform is Pop!_OS and the target is exactly `SourcesListD("system-security.sources")`; open with `create_new` and mode 0600; `EEXIST` aborts the whole apply with nothing written. Any other `create_new` target, and `create_new == false` on a file that does not exist, aborts before the first open (7.8) |
| backup root | not touched | created with mode exactly 0700, then verified after creation (owner root, mode 0700, not a symlink); a pre-existing directory that fails those checks aborts (7.6) |
| ordering | none | validate every file, then back up every file, then write every file; a backup failure aborts before the first write |

### 5.5 Why the boundary matters

If the privileged phase re-reads anything and acts on it without comparing to the plan, three failures become reachable that are otherwise closed: a local user's last-moment file substitution becomes a root write (B2); a mirror's forged date becomes a written URI (3.9); and the bytes written stop matching what the user confirmed. The handoff shape above closes all three with one mechanism — a closed schema plus digest comparison against current disk state — and it makes the security property reviewable without reading the write implementation.

## 6. Security acceptance tests that must exist before D9 merges

Checklist for a human reviewer. Every test runs in a disposable Ubuntu container with an isolated `/etc/apt` fixture and `--network none` where the case permits; none may touch a host path. "Fails when" describes the assertion, so a reviewer can check the test is real.

- **S1 — `--apply` never writes, whatever its uid.** As uid 1000: exit 3, no file opened, message names the exact `sudo` command. As root, with `--network none` and `/usr/bin/gpgv` removed: exit 3, the message names the same two-invocation command and says the network phase must not run as root, no DNS/TLS request is attempted, no external process is spawned, and every `/etc/apt` target digest is identical before and after. *Fails when* the process writes anything, exits 0/1, omits the command, or a root run reaches the network or a `gpgv` spawn.
- **S2 — Root phase without a plan.** `--plan-in -` with empty stdin, truncated container, bad magic, or a future `format`: exit 2, no file opened. *Fails when* any partial parse reaches a target.
- **S3 — Plan schema hostility.** Fixtures: unknown tag, duplicate tag, 5 MiB field, `..` inside a `FileName`, `system-security.sources/../sources.list`, absolute name, NUL byte, non-UTF-8 codename, `producer_uid = 0` on an apply plan, `producer_uid = 1000` on a restore plan, a `Restore` plan whose `backup` is `../..`, an absolute path or a name with a `/`, an `Apply` plan carrying a `manifest_digest` or a `Restore` plan carrying a `proof` or a uri, a `create_new` target other than `system-security.sources` (for example `SourcesListD("evil.list")`), and `create_new == false` naming a file that does not exist. Every one is refused before any open; the `producer_uid` cases exit 1 naming `producer_uid` with nothing opened. *Fails when* one is accepted or produces a path outside `/etc/apt` or outside `/var/backups/apt-scout/<id>/`.
- **S4 — Path traversal in the manifest (`--restore`, D10 scope).** A manifest naming `/etc/shadow`, `../sources.list`, `sources.list.d/../../etc/passwd`, a tuple of `/etc/apt/sources.list` as a symlink, or an absolute path: refused, nothing read, nothing written. *Fails when* the path resolution is done by string prefix alone — prove it by including `/etc/apt/../etc/shadow` and `/etc/aptx/sources.list`.
- **S5 — Symlink target.** `sources.list` and each `.list`/`.sources` are replaced by symlinks to a canary outside `/etc/apt`; and `sources.list.d` itself is a symlink to an attacker directory. Each case: refused with the reason, canary unchanged. *Fails when* the write follows the link.
- **S6 — TOCTOU substitution.** With the plan validated, replace a target between validation and write (test hook: a variant that pauses after validation). Expected: write aborts on the dev/ino or digest mismatch, target unchanged. *Fails when* the write proceeds on the substituted inode.
- **S7 — `:ro` → `:rw` mount.** Run the privileged phase against `/etc/apt` mounted read-only: exit 1 with the OS error verbatim, no temp file left behind, backup directory either absent or untouched, and a second run does not treat the leftover as a backup. *Fails when* the error is swallowed or a temp file survives.
- **S8 — Backup failure aborts before any write.** Make the backup directory or the timestamp directory unwritable (mode 0500) and make the first backup write fail; assert the order by digesting every target before and after: all identical, and the program's output names which file failed. *Fails when* one target changed.
- **S9 — Pre-created backup directory.** `/var/backups/apt-scout` owned by uid 1000 or mode 0777 or a symlink: hard abort with the reason, nothing written inside it. *Fails when* `create_dir_all` semantics let it through (the old self-heal rule allowed this; `SPEC.md` Errors and self-healing now requires the abort).
- **S10 — Rollback.** Apply succeeds, then `--restore` from the newest backup: every file byte-identical to the pre-apply fixture (`cmp`, mode, uid/gid), including removal of `system-security.sources` on Pop!_OS. *Fails when* any byte, mode, or owner differs, or the created file survives.
- **S11 — Rejection of paths outside `/etc/apt`.** A plan whose `Target` is `SourcesListD` with a name that resolves outside, plus a `--restore` manifest pointing at `/usr/bin/gpgv`: exit 1, message names the offending path, no open for write. *Fails when* the privileged phase opens any path outside the closed enum's two roots.
- **S12 — Hostile `PATH`/`HOME`/`TMPDIR`.** Run the privileged phase with `PATH=/tmp/evil`, `HOME=/tmp/evil`, `TMPDIR=/tmp/evil-world-writable`, each containing a fake `gpgv`, a fake `sudo`, and a `.gnupg` directory; assert no `/tmp/evil/*` binary is executed (verified by the fake binaries creating a marker) and that verification results come from the plan, not from a re-run. *Fails when* any marker appears or any `TMPDIR`-derived path is used by the privileged phase.
- **S13 — Environment not inherited.** Start the privileged phase with `sudo -E`-style environment (`http_proxy`, `APT_CONFIG`, `LD_PRELOAD`): all ignored, no network syscall attempted (assert with `--network none`). *Fails when* the run behaves differently with them set.
- **S14 — Privileged phase makes no network request and runs no external process.** With `--network none` and a `strace`-free check: run the phase on a valid plan and assert success; then re-run with `/usr/bin/gpgv` removed and assert success again. *Fails when* removing `gpgv` changes the outcome — that would prove the privileged phase re-verifies.
- **S15 — The D6f P1 regressions gate the write, not only the display.** `noble-updates-InRelease.prefixed` (forged `Date:` before the armour): read-only run ranks it under the current tolerance, write run refuses to select it and refuses a plan naming it. Plus a plan claiming `Verified` for an unverified mirror does not parse, and a plan whose content or manifest digest does not match what the privileged phase recomputes is refused before the first write (G1, G2). *Fails when* either path accepts it.
- **S16 — Disposable-container rule, which the harness must gain before any privileged case exists.** Nothing enforces this today, because no privileged case exists: `scripts/docker-test.sh` mounts only the binary, read-only, into a container it starts (`scripts/docker-test.sh:28`), and touches `/etc/apt` only inside those containers (`scripts/docker-test.sh:86`); it has no `/etc/apt` override of any kind and no privileged-case path. Before the first privileged case, the script must refuse to start one outside a container it created itself, and must run it against a container-local `/etc/apt` reached through a path compiled into test builds only: a build-time constant, never a runtime variable, because the privileged phase reads no environment variable (`SPEC.md` Privilege boundary, S12, S13), and never a bind mount of the host's `/etc/apt`. That constant does not exist yet and is a constraint on the test build, not a mechanism to rely on: a release build must contain no such path override. *Fails when* a privileged case can be pointed at a host path, or a release binary accepts anything but the default `/etc/apt`.
- **S17 — Pre-existing Pop!_OS security file.** On a Pop!_OS fixture, `/etc/apt/sources.list.d/system-security.sources` already exists (both as a regular file and as a symlink): `EEXIST` aborts the whole apply, every target digest is identical afterwards, the pre-existing file is byte-identical with its mode and owner unchanged, and no half-written backup directory is left behind. *Fails when* the file is overwritten, followed as a symlink, or any other target changed.

## 7. Decision record

Ruled 2026-11-12 by the user, relayed by the parent orchestrator. These are rulings, not proposals: the body above and `SPEC.md` are read as decided. What remains is not implementation only: the deltas Appendix A marks `PENDING` are not in `SPEC.md` yet. Each entry names the section that carries the ruling into behaviour and the tests that check it.

- **7.1 Self-reexec — Option A.** No self-reexec, no setuid bit, no internal `sudo`. A non-root `--apply` writes nothing, exits 3, and prints the exact two-invocation command. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 4.2 and 4.5; `SPEC.md` Privilege boundary bullet 4, Apply bullet 1, CLI; S1.
- **7.2 Plan transport — inherited fd only.** The plan crosses on stdin and nowhere else; no plan-path argument exists at all. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.3; `SPEC.md` CLI; S2.
- **7.3 Plan author.** The root phase refuses a plan produced by uid 0 when it is an apply plan, so running the whole command under `sudo` cannot put the network phase in the root path; by the same rule it refuses a restore plan produced by any non-root uid, because such a run cannot have read the root-only backup (G1). Both directions fail closed. `producer_uid` is honest-producer metadata, not an anti-tamper control (5.4): a process running as the same uid can claim any value, so this ruling catches the accidental `sudo` run and, for a restore, a plan that could not have been built from the backup. DECIDED 2026-11-12 by the user (via the parent orchestrator), scoped by operation in G1. Carried by 4.6, 5.2 and 5.4; `SPEC.md` CLI; S3 (`producer_uid = 0`).
- **7.4 D6f P1 #1.** For a write, `Date` is parsed only inside the cleartext-signed region; any non-whitespace before `-----BEGIN PGP SIGNED MESSAGE-----` is rejected. Read-only ranking keeps its current tolerance. DECIDED 2026-11-12 by the user (via the parent orchestrator). Implemented by D6f, merged as PR #29 (`8bbfa72`); this document does not change code. Carried by 3.9; `SPEC.md` Pipeline step 8; S15.
- **7.5 D6f P1 #2.** One proof value — signed-region date plus verification result, private constructor — is carried through `fresh::Probed` and the baseline; `fallback_baseline` accepts only verified candidate dates; the write fails closed when nothing verifies. DECIDED 2026-11-12 by the user (via the parent orchestrator). Implemented by D6f, merged as PR #29 (`8bbfa72`); this document does not change code. Carried by 3.10 and 5.2; `SPEC.md` Freshness trust boundary (Appendix A item 4); S15.
- **7.6 Backup root.** Keep `/var/backups/apt-scout`. A pre-existing path that is not root-owned, is not mode 0700, or is a symlink is a hard abort with nothing written; the `create_dir_all` self-heal no longer covers it. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by B5 and 5.4; `SPEC.md` Errors and self-healing; S9.
- **7.7 `--restore`.** Restore uses the same plan and the same privileged phase as `--apply`: one privileged entry point, and restore is another kind of plan. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.1 and 5.4; `SPEC.md` Apply and CLI; S4, S10. The plan shape a restore needs is defined in 5.2 and ruled in G1: an operation discriminator, the backup id and the manifest digest, and both invocations root.
- **7.8 Pop!_OS new file.** `/etc/apt/sources.list.d/system-security.sources` is created with exclusive create and mode 0600; `EEXIST` aborts the whole apply, so a file the run did not plan is never overwritten. `create_new` is accepted only for that one target on Pop!_OS, and `create_new == false` on a nonexistent file aborts before the first open, so a plan cannot create any other file in `/etc/apt`. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by B8, 5.2 and 5.4; `SPEC.md` Apply (file creation bullet), Errors and self-healing; S17, S3.
- **7.9 Plan encoding.** Length-prefixed tagged container: no escaping, a pure parser, hostile-input unit tests. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.3; S2, S3.

Appendix A records what each ruling changed in `SPEC.md`, and the owner of every delta that is not yet applied. Thirteen rulings being decided is not D6e being closed: D6e stays open until the independent security review of this record returns PASS.

---

## 8. The three gaps the first review left open, and their rulings

The first independent security review left three gaps open. All three are ruled by the user (2026-11-12, relayed by the parent orchestrator). Each entry states the decision, why it is fail-closed, the abuse path it closes, and the tests that check it. No gap is left open, so this section no longer blocks D9; the independent security review's PASS verdict is what remains for D6e.

**G1 — the restore plan, and who may build one.** Source: audit P1-3 and P1-4. Ruled: `WritePlan` gains an operation discriminator (5.2), and a restore plan carries the backup id plus the digest of the manifest it was built from — never the restored bytes. The privileged phase reads `/var/backups/apt-scout/<id>/MANIFEST` by a path it builds itself, compares the digest, aborts on a mismatch, and only then parses the manifest and validates every target it names (5.4). The single privileged entry point stays (7.7): a restore is another kind of plan consumed by the same `--plan-in -` phase, and no second root code path re-derives a decision from a plan-less state.

Consequences, each fail-closed:

- The backup directory is 0700 root-owned and the manifest 0600, so such a plan can only be produced by a run that holds root. `--restore --plan-out -` without root reads nothing, prints `sudo apt-scout --restore --plan-out - | sudo apt-scout --plan-in -`, and exits 3; as root it reads the newest backup by fixed path, emits the plan, and prints the matching `--plan-in -` command. Both invocations of a restore are root, which does not weaken the apply model: a restore has no network phase and no third-party input, and its trust boundary is the backup's own 0700/0600, not a fetched body (5.1).
- 7.3's `producer_uid = 0` refusal is scoped by the discriminator and fails closed in both directions: an apply plan is refused when produced by uid 0, a restore plan when produced by any non-root uid (5.4).
- The plan is never the source of truth for what is written. The privileged phase re-derives the target set and every per-target digest from the manifest itself, so the plan's values are an assertion that must match exactly: a stale or partial plan cannot apply a subset of the backup and still "match".

Abuse path closed: a coder implements 7.7 literally and either reads the backup as root without a plan — a second root code path that re-derives decisions — or relaxes the backup so that an unprivileged planner can read it, which defeats B5 and lets a local user choose what root restores. Tests: S4, S10, S11, and the `Restore` cases in S3.

**G2 — plan integrity: content, not producer.** Source: audit P1-6. Ruled: the plan carries the digest of every target's new content, plus the manifest digest for a restore, and the privileged phase recomputes both and aborts on any mismatch before its first write (5.4). What that buys, stated without inflation: a process running as the invoking uid is *inside* the trust boundary, because it can invoke the privileged phase itself with a plan it wrote. The guarantee is content binding — root writes the bytes the digests name, to the targets the closed enumeration allows, over a disk state that still matches — and never producer identity. `producer_uid` stays honest metadata (7.3), and no digest makes it a MAC, so S15's second half is restated to the one thing the design can implement: a plan whose digests do not match is refused.

Abuse path that this ruling accepts, and records: any process running as the user — a hostile `apt-scout` earlier in the user's `PATH`, a traced producer — can hand root shape-valid plan bytes and conscript it into writing them into apt sources, which the next `apt` operation installs from. Accepted, because the alternative is root re-deriving or re-verifying untrusted input, which `SPEC.md` Privilege boundary forbids and S14 tests against. Tests: S3, S6, S15.

**G3 — one binary, out of scope for v1.** Source: review P2-4. Ruled, in the user's own framing: the program is not complicated, so it **is one binary**. The second-artifact trigger and its line-count threshold are removed outright — not left reopenable by a human, not restated as a signal a reviewer may raise. 4.6 now states that a split of the privileged phase into a second artifact is out of scope for v1, and that any future split would be a new product decision taken by the user with the threat model 4.4 requires, never something a size measurement or a review finding can trigger. `SPEC.md` Privilege boundary and threat model carries the same statement for behavior. Abuse path closed: an agent reading the size of the privileged path as authorization to ship a second root artifact that no audit covers. No test of its own; 4.4's C-spawned-by-root entry stays rejected.

---

## Appendix A — SPEC delta ledger

State of every delta the rulings in section 7 require. APPLIED names the `SPEC.md` section that now carries the requirement; PENDING names the owner. No item is left unowned.

1. **APPLIED — `SPEC.md` "Privilege boundary and threat model", bullet 4.** Replaced "Do not use privileged self-reexec of the current executable until D6e approves a concrete trust mechanism." with "The program never re-execs itself privileged, never invokes `sudo`, and is never installed setuid. Privilege is obtained only by the user running the program as root. The root invocation is a separate mode that reads a validated plan and performs file operations only; it makes no network request, runs no external process, resolves no command through `PATH`, and reads no environment variable."
2. **APPLIED — `SPEC.md` "Apply", bullet 1.** Replaced "Do not implement self-reexec through `sudo` or another privileged helper until the privilege-boundary design passes D6e security audit." with "Discovery, verification, rewriting, and confirmation produce a plan that is the root phase's only input. The plan is a closed schema: it names files through a fixed enumeration, carries the exact bytes to write, carries the verification result for the selected mirror and the freshness baseline, and carries the digest, mode, and owner of every file it was computed from. The root phase aborts, before its first write, on any missing field, any mismatch with the file on disk, and any plan it did not fully validate."
3. **APPLIED — `SPEC.md` "Pipeline", step 8.** A `Date:` is trusted only inside the cleartext-signed region: only whitespace-only lines may precede `-----BEGIN PGP SIGNED MESSAGE-----`, which must start its own line, the armour headers are skipped to the first blank line, and the search stops at `-----BEGIN PGP SIGNATURE-----`. A prefixed body still has a good signature, so its status stays `verified` and the `Date` read outside the region is never a write baseline and never authorizes a write; read-only ranking keeps using it. The sentence this ledger first recorded ("bytes before ... must be ASCII whitespace only") was tightened when D6f merged as PR #29; the merged sentence is the one `SPEC.md` carries today, and this item names it so the ledger does not describe text that no longer exists.
4. **APPLIED (D6f, PR #29) — `SPEC.md` "Non-functional", "Freshness trust boundary".** States that only a `Date` parsed inside the cleartext-signed region is ever trusted; that ranking may use a `Date` whose status is `invalid` or `unavailable` while only a `Verified` `Date` parsed inside the region is usable by a write; that the write baseline is the canonical reference when its own `verified_updates()` is `Some`, otherwise the newest candidate whose own `verified_updates()` is `Some`, and that a write fails closed when neither exists; and that trusted freshness dates are a value only a `Verified` status can construct (`VerifiedDate`). The write-gate types that must still be built — an unforgeable status, the verified-plan type, the closed plan schema — belong to D9 and are shaped by 5.2 and G2.
5. **APPLIED — `SPEC.md` "CLI" (the two new table rows `--plan-out`/`--plan-in`, the `--apply` behaviour bullet, the exit-code list), "TUI" (zero-effort flow steps 2–3, the extra-steps bullet, the applying-from-the-TUI bullet), "Apply" (the `--restore` bullet).** The `--apply` and `--restore` table rows are unchanged; the rows added are `--plan-out -` and `--plan-in -`. Applied narrower than the draft below, because 7.2 admits no plan-path form: `--plan-out -` prints the plan on stdout, `--plan-in -` reads it from stdin, and any value other than `-` is refused. `--apply` never writes whatever its uid: without root it opens no apt file, exits 3 and prints `apt-scout --apply --plan-out - | sudo apt-scout --plan-in -`; as root it performs no network access, runs no external process, opens no apt file, prints the same command and exits 3, so there is no one-process root apply. The privileged mode is reached only through that second invocation and refuses a plan produced by uid 0. TUI step 2 `Enter` emits the plan and prints the command, step 3 reports `plan written, run <command>`. `--restore` builds its plan from the newest backup and enters the same privileged mode, so one entry point serves both. Draft: "Add the plan flags to the table (`--plan-out <path|->`, `--plan-in <path|->`) and state that `--apply` without root exits 3 and prints the exact command, and that the privileged mode is reached only through a second, explicit invocation."
6. **APPLIED — `SPEC.md` "Errors and self-healing", the self-heal bullet.** "Missing own directories (`~/.cache/apt-scout`, `/var/backups/apt-scout`): create with `create_dir_all`." was replaced with "Missing own cache directory: create with `create_dir_all`. `/var/backups/apt-scout` is created by the root phase and verified after creation to be a directory, owned by root, mode 0700, and not a symlink; a pre-existing path that fails any of these is a hard abort with nothing written."
7. **APPLIED — `SPEC.md` "Errors and self-healing", the Docker bullet.** The drafted section name "Testing" is wrong: the anchor sentence ("Docker integration tests for source discovery/apply/restore must use disposable Ubuntu containers and isolated `/etc/apt` fixtures.") lives in Errors and self-healing and is present verbatim there, so the text was appended to it: "Privileged-write tests cover: non-root refusal, plan schema hostility including path traversal and oversize fields, symlinked targets and a symlinked `sources.list.d`, substitution between validation and write, a read-only `/etc/apt` mount, backup failure aborting before any write, byte-exact restore of content/mode/owner, rejection of paths outside `/etc/apt`, hostile `PATH`/`HOME`/`TMPDIR`, and a run that succeeds with `gpgv` removed, proving the privileged phase does not re-verify." Extended later in the same pass with `--apply` refusing at a root uid, a `create_new` target other than `system-security.sources`, and a pre-existing `system-security.sources` aborting the apply. The same pass added to `SPEC.md` "Testing", in the `scripts/docker-test.sh` bullet, that the privileged cases address `/etc/apt` through a path compiled into test builds only, because S16 must not require a runtime environment variable that the privileged phase is forbidden to read.
8. **PENDING (D9) — `GLOSSARY.md`.** Owner: D9, the milestone that introduces the privileged phase; this document does not edit `GLOSSARY.md`. Three terms, vocabulary only: **Plan** — the validated description of a pending apt change, carrying the bytes to write and the proof they came from verified data; it is the only input to the privileged phase. **Privileged phase** — the part of a run that holds root; it performs file operations and nothing else. **Write proof** — the value that ties an `InRelease` date to a successful signature verification.
9. **APPLIED — `SPEC.md` "Apply", new file-creation bullet; "Errors and self-healing", the self-heal prohibition.** Added to Apply: "`/etc/apt/sources.list.d/system-security.sources` is created with `create_new` and mode 0600, and `EEXIST` aborts the whole apply with nothing written. A planned write that creates a file is accepted only when the platform is Pop!_OS and the target is exactly `sources.list.d/system-security.sources`; a plan that creates any other file, or replaces a file that does not exist, aborts before the first open." The Errors and self-healing prohibition ("never create files in `/etc/apt` that did not exist") was reworded so that it is about self-healing only and names that one planned creation, and the Apply bullet carrying ruling 8's intent now states the constraint in the place a coder reads. Carried by 7.8; gated by S17 and S3.

10. **APPLIED — `SPEC.md` "Apply", bullet 1 and the `--restore` bullet; "CLI", the `--restore` and `--plan-in -` rows; "Errors and self-healing", the Docker test list.** G1: the plan carries an operation discriminator; an apply plan carries the selected URI and the proof, a restore plan the backup id and the manifest digest; `--restore` without root reads no backup, prints `sudo apt-scout --restore --plan-out - | sudo apt-scout --plan-in -` and exits 3; the privileged phase reads the manifest itself, compares the digest, and only then validates targets and refuses any disagreement. The CLI row for `--plan-in -` now refuses an apply plan produced by uid 0 and a restore plan produced by a non-root uid. The Docker list gains a restore plan whose manifest digest does not match, and a restore plan whose backup id is a path; S3 gains the restore-side producer and `BackupId` fixtures.
11. **APPLIED — `SPEC.md` "Apply", bullet 1; "Privilege boundary and threat model", new bullet.** G2: every target's new content carries a digest the privileged phase recomputes, plus the manifest digest for a restore, and a plan whose digests do not match is refused; the privilege-boundary bullet states that the guarantee is content binding, not producer identity, because a process running as the invoking uid can invoke the privileged phase itself. S15's second half is restated in section 6 to match.
12. **APPLIED — `SPEC.md` "Privilege boundary and threat model", new bullet.** G3: the privileged phase is one binary, a mode of the same executable; a separate privileged artifact is out of scope for v1, and splitting it would be a new product decision rather than a threshold.

---

## Appendix B — accepted risks and items owned elsewhere

Recorded so nothing here is dropped silently; none of these is an open decision.

- The throughput sample is attacker-controlled (`src/probe.rs:153–156`) and can decide the ranking. Accepted: it cannot authorize package installation, and `SPEC.md` Pipeline step 9 already calls throughput relative. Owner: D9, with no SPEC delta.
- Root printing an unescaped mirror URL or error string into a terminal (B7) is only a hazard once a privileged phase prints anything. The mitigation belongs to D13, not D9.
- `reader.join()` is unbounded after the timeout kill in the verifier (`src/verify.rs:212–232`), and the oversized-body case reports `Unavailable` rather than `Invalid`. Both stay in D6f's P2 list; neither changes what a write may trust.
- `--list` is accepted and ignored (`src/main.rs:44`); that is a CLI conformance gap for D11, not a security issue.
