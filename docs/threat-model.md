# D6e — Privilege boundary threat model and privilege-escalation decision

Status: **decided.** This is the privilege-boundary decision record for apt-scout: the threat model, the self-reexec ruling, the trusted root handoff, the tests that gate D9, and the state of every `SPEC.md` delta the rulings require.
Base: `origin/develop` = `8c0c354`. Roadmap item: D6e (blocks D9, D10, D11, D12, and every later write).
Authority: `ROADMAP.md` D6e/D6f/D9–D12, `SPEC.md` "Privilege boundary and threat model", "Apply", "Pipeline", "Testing", and `AGENTS.md:105`.

This document records four things: what may cross into a privileged phase, that the program never obtains privilege for itself, which tests must exist before D9 can merge, and what a write may trust about an `InRelease`. Section 7 is the decision record; Appendix A states whether each `SPEC.md` delta is applied and, where it is not, who owns it. A superseded ruling is replaced in place, never annotated.

---

## 0. Reading basis and current state

Read for this document: `AGENTS.md` (whole file; `AGENTS.md:105` privilege-security gates), `ROADMAP.md` (whole file), `SPEC.md` (whole file, 188 lines), `GLOSSARY.md`, `src/verify.rs` (1–350), `src/http.rs` (1–250), `src/probe.rs` (1–180), `src/fresh.rs` (78–120), `src/sources.rs` (1–60, 380–520, 795–815), `src/main.rs` (1–120, 215–340, 485–585), `src/mirrors.rs` (245–300), `Cargo.toml`.

Facts that constrain the handoff. Each is the current code at this base, not a plan:

1. **There is no write path.** No `fs::write`, `File::create`, `rename`, or `set_permissions` exists in any non-test code path. The only files the program creates are the verification temp directory and temp file (`src/verify.rs:129–149`) and it removes both.
2. **No root check exists.** `geteuid`/`getuid` is not called anywhere; exit code 3 ("not root", `SPEC.md` Non-functional, exit codes) is unimplemented. `main` maps only the codes `run()` returns (`src/main.rs:572–580`; codes 1/2/4 at `src/main.rs:96–121`).
3. **`--apply`, `--restore`, `--yes`, `--no-tui`, `--refresh` do not exist.** `parse_args_from` accepts exactly `--list`, `--https-only`, `--no-geo`, `--country <CC>`, `--top <N>`, `-h/--help` (`src/main.rs:47–77`), and rejects anything else (`src/main.rs:75`). `--list` is accepted and ignored (`src/main.rs:47`). The whole run is read-only.
4. **No TUI exists.** `Cargo.toml` dependencies are `ureq` only; `ratatui`/`crossterm` arrive in D13. Terminal input is not an input today.
5. **The only environment variable the code consumes is `TMPDIR`,** indirectly, through `std::env::temp_dir()` in `private_temp_dir` (`src/verify.rs:54`). `HOME`, `PATH`, and `XDG_*` are never read. `std::env::args()` is the CLI source (`src/main.rs:31`).
6. **`InRelease` is verified with a fixed binary and keyring:** `/usr/bin/gpgv` (`src/verify.rs:15`), `/usr/share/keyrings/ubuntu-archive-keyring.gpg` (`src/verify.rs:16`), both `is_file`-checked before use (`src/verify.rs:101–127`), spawned with `env_clear()` plus `LC_ALL=C` (`src/verify.rs:156–166`), `--homedir` set to the private 0700 directory (`src/verify.rs:60–64`, `src/verify.rs:160`), stdin nulled (`src/verify.rs:165`), 10 s timeout (`src/verify.rs:20`), 1 MiB status cap (`src/verify.rs:21`), and `Verified` requires exit success plus `GOODSIG` plus `VALIDSIG` and no `BADSIG`/`ERRSIG`/`NO_PUBKEY` (`src/verify.rs:250–257`).
7. **The freshness `Date` is parsed outside the signed region.** `fresh::release_date` scans every line of the whole body for `Date:` and stops only at `-----BEGIN PGP SIGNATURE` (`src/fresh.rs:81–90`); `date_of_verified` passes it the entire downloaded body (`src/probe.rs:72–86`), and `fetch_reference` does the same for the baseline (`src/probe.rs:114–131`).
8. **"Verified" has no representation where a write would need it.** `fresh::Probed` carries `url`, `lag_secs`, `throughput_bps`, `latency_ms`, `same_as` and no signature (`src/fresh.rs:104–111`); the baseline takes `reference.updates` without consulting `reference.updates_signature` and falls back to `fallback_baseline(...)` over every `Ok` outcome (`src/main.rs:234–244`, `src/main.rs:492–504`); the reference signature is printed but never gates anything (`src/main.rs:317–322`). `SignatureStatus` is a public enum constructible anywhere in the crate (`src/verify.rs:24–36`).

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
| A6 | `InRelease` `Date` | `src/fresh.rs:81–90` over the whole body | The mirror, outside the signed region (finding 7, D6f P1) | Yes — decides which mirror is written |
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
- A6: `Date` is read from the whole body, not the signed region (see 3.9).
- No `Verified` representation reaches the ranking data (see 3.10).
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
- The backup root must be verified after creation: `symlink_metadata` says directory, `uid == 0`, mode has no group/other bits, and the path is not a symlink. A pre-created `/var/backups/apt-scout` by uid 1000 must be a hard abort, not a `create_dir_all` no-op. `SPEC.md` Errors and self-healing now states that exception (Appendix A item 6): the `create_dir_all` self-heal covers the cache directory only.
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

### 3.9 D6f P1 #1 — the freshness `Date` is parsed outside the signed region

What the code does: `date_of_verified` downloads the whole `InRelease` and calls `fresh::release_date` on the entire body before calling the verifier (`src/probe.rs:72–86`). `release_date` takes the first line starting with `Date:` and stops only at `-----BEGIN PGP SIGNATURE` (`src/fresh.rs:81–90`). `fetch_reference` does the same for the baseline (`src/probe.rs:114–131`). `gpgv` ignores everything before `-----BEGIN PGP SIGNED MESSAGE-----`, so a mirror can prepend `Date: <chosen>` to a genuinely signed body and still be reported `verified` with an attacker-chosen date.

Ruling: **for a write, the date must come from inside the cleartext-signed region, and the region must be located by the parser, not by the verifier.**

Required behavior, both sides:
- Bytes before `-----BEGIN PGP SIGNED MESSAGE-----` must be ASCII whitespace only. Anything else makes the `InRelease` unusable for a write (`Invalid`), and it stays usable for read-only ranking under the current `SPEC.md` Freshness trust boundary sentence ("ranking may display and use an `InRelease` `Date` even when signature verification is `invalid` or `unavailable`").
- The `Date:` header is taken from the cleartext between the armour headers (after the `Hash:` line) and `-----BEGIN PGP SIGNATURE-----`. A `Date:` occurring after the signature block is not the release date.
- A write requires: parse succeeded inside the signed region *and* `SignatureStatus::Verified`. Both facts travel together (section 5), so no later code can combine one without the other.
- Regression fixture: `tests/fixtures/signed/noble-updates-InRelease.prefixed` — the existing fixture with a forged `Date:` prepended — must produce a date equal to the genuine one for a write, or an outright refusal; it must never produce the forged date. The hand proof in D6f (`gpgv` exits 0 with `GOODSIG`/`VALIDSIG` on the prefixed copy) is the reason this test is mandatory.

What this means for a write: without this, "verified" does not mean what the write gate needs it to mean, and every other control in this document inherits a forgeable input. This is why it blocks D9 rather than merely degrading read-only output.

### 3.10 D6f P1 #2 — no representation of "verified" for the baseline and the selection

What the code does: `fresh::Probed` has no signature field (`src/fresh.rs:104–111`); `main` builds the baseline from `reference.updates` without checking `reference.updates_signature`, and from `fallback_baseline` over every `Ok` probe outcome (`src/main.rs:234–244`, `src/main.rs:492–504`); the reference signature is printed and discarded (`src/main.rs:317–322`). `SignatureStatus::Verified` is a public unit variant (`src/verify.rs:24–32`), so any code in the crate can mint it.

Ruling: **"verified" must be a value, not a string and not a bool, and the write gate must be able to accept only that value.**

Required behavior:
- One constructor produces the proof: the verification pass itself, in the unprivileged phase, in the same run. `SignatureStatus::Verified` becomes unconstructible outside `verify` (private module constructor; `is_verified()` stays for display).
- The proof carries the parsed date and the fact that the date came from the signed region (3.9), so the two P1s close into a single value that cannot be assembled from parts.
- `fresh::Probed` carries the proof's status, and the baseline is `Verified(date)` or absent. `fallback_baseline` contributes a date only from candidates whose `-updates` `InRelease` was verified in this run. A forged future date is then impossible rather than merely implausible: a genuine Ubuntu signature cannot cover an attacker's `Date`.
- If no candidate verifies, the write fails closed with an explanation naming the cause (missing `gpgv`, missing keyring, every mirror `invalid`), and read-only output is unchanged.

What this means for a write: the accepted rule in `SPEC.md` Freshness trust boundary ("the selected mirror's `-updates` `InRelease` and the canonical reference used as freshness baseline must both be `Verified`") has no representation today, so it cannot be enforced by construction. Adding the type is cheaper than adding checks that must remember to run in the right order.

## 4. Self-reexec: the explicit decision

### 4.1 The constraint that decides it

A process cannot obtain privilege for itself, so "self-reexec" is shorthand for one of three mechanisms, and they are not equivalent:

1. exec itself under `sudo` — the program invokes `sudo`. Forbidden by `AGENTS.md:105` ("Never add or assume `sudo` self-reexec is safe") and by `SPEC.md` Privilege boundary bullet 4, which states that the program never re-execs itself privileged and never invokes `sudo`. The ban stands in the ruled design: a program that execs `sudo` converts every path it controls into a root path, and inherits `sudo`'s environment policy as its only defense.
2. install a setuid-root bit — the binary becomes a local privilege-escalation candidate by definition. Any local user can run the write phase. The only thing standing between them and root file writes is the program's own validation of an input that the same local user supplies.
3. ask the user to run the second phase as root — the mechanism the user's own `sudo` provides, with no privilege manufactured by the program. This is the ruling (7.1).

Mechanism 3 is the ruling (7.1); mechanisms 1 and 2 are the rejected forms analysed in 4.3 and 4.4. The question D6e answered is what the second, root-only invocation is allowed to contain, and the answer is section 5: a plan, and nothing else.

### 4.2 The ruling — no self-reexec: refuse and print the exact command

The program never execs anything privileged. `--apply` as a non-root uid prints the exact two-invocation command and exits 3. The privileged invocation is a separate, narrow mode that reads a validated plan (section 5) and performs only file writes.

- **Buys:** the smallest privileged surface that satisfies `SPEC.md` Privilege boundary bullet 3 ("Root is limited to a small final operation over a previously validated plan"); no setuid bit, no internal `sudo`, no second executable to package; the privileged path is reachable only by an explicit human action with a visible command line; the unprivileged phase keeps every network request, `gpgv` invocation, and parse, which is what bullet 3 requires.
- **Costs:** two invocations, so the plan must be transportable (section 5) — which every option needs anyway once network access may not run as root; the user has to read and copy a command.
- **How it fails:** visibly. Wrong uid → exit 3 with the command, no file opened. Missing plan → exit 2. Plan invalid → exit 1 with the failing field, nothing written. No partial state is reachable, because the first thing the privileged mode does is validate the whole plan before opening any target.
- **What an attacker gains:** nothing new beyond what they already have if they can run the program as root themselves. A hostile local user cannot cause a privileged run; they can only supply plan bytes, which the privileged phase validates against a closed schema and against on-disk digests.
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
- **C-spawned-by-root:** the user runs the write mode as root; it forks a child that reads the plan from a pipe and performs the writes. If the second binary is separate, it contains no `ureq`, no `gpgv` spawn, no TUI, no DNS — the privileged image is a few hundred lines and its interface is a byte schema. Not adopted for v1; this is the structure 4.6 holds in reserve if the privileged code outgrows a small final operation.
- **C-setuid-helper:** the unprivileged parent spawns a setuid helper. Same rejection as Option B: it manufactures privilege from an unprivileged caller.

- **Buys:** structural isolation that a reviewer can see in the dependency graph rather than having to trust a discipline. The interface between the two halves is enforced by the OS, and the privileged binary's "does not make network requests" claim becomes checkable (`cargo tree` on a two-crate workspace) instead of checkable-by-grep.
- **Costs:** a second binary in `.deb` assets (`Cargo.toml` `package.metadata.deb.assets`, currently one entry) and in the release tarball, a workspace split or a feature-gated second `[[bin]]`, and two artifacts to keep in version lockstep (the plan carries a format version for that reason). It also doubles the amount of packaging and release surface tested before D9.
- **How it fails:** like Option A, plus one new failure: a version skew between the two binaries. Mitigated by requiring an exact plan-format version match and refusing otherwise.
- **What an attacker gains:** nothing beyond Option A. The helper is reachable only from root.
- **Testable:** fully, in the same containers; the container test must additionally assert that the privileged binary contains no network-capable dependency, which is a stronger and cheaper claim to verify than a grep review.

### 4.5 Ruling and reason

**Option A. DECIDED 2026-11-12 by the user (via the parent orchestrator). C-spawned-by-root's structure stays the condition to revisit (4.6).** Specifically: no self-reexec anywhere in the codebase; the unprivileged run computes and prints the plan and the exact `sudo` command; `--apply` on a non-root uid exits 3 without opening a file; the privileged invocation is a narrow mode that reads the plan from an inherited fd, validates it fully (section 5), verifies every on-disk digest, and writes — and never makes a network request, never runs `gpgv`, never invokes a shell, and never resolves a command through `PATH`.

Reasoning, in order of weight:
1. It is the only option that keeps `SPEC.md` Privilege boundary bullet 3 true without a setuid binary or an internal `sudo` call. Options B and C do not add privilege; they add code that pretends to.
2. The security property that matters is not "one process" but "the root process's input is a closed schema validated against disk state". That property is identical in A and C-spawned-by-root, so the extra artifact buys isolation of *code*, not of *authority* — and it costs packaging surface before D9, when the privileged code is still small.
3. Fewer concepts: one binary, one format, one refusal path, one test matrix. Option B's token and second code path are concepts that exist to solve a problem Option A does not have.
4. Testability: the refusal path, the plan validation, and the write path are all reachable from a container command line, with no setuid install step and no nested privilege transition to reason about.

### 4.6 Triggers that reopen this ruling

- If the privileged code outgrows a small final operation — for example if it acquires its own parsing, its own inventory pass, or its own verification — the structural split of C-spawned-by-root becomes the cheaper control, because the discipline that keeps `ureq` out of the privileged path stops being visible. Trigger: read `src/apply.rs` as a whole and count the copies of decisions the unprivileged phase already made; above roughly 500 lines of privileged code, switch to two binaries.
- If the user-facing cost turns out to dominate: a two-invocation flow that people get wrong (for example by running the whole command under `sudo`, so the network phase runs as root) is worse than a documented one-command flow. Mitigation inside the ruling: the refusal message prints the exact command *and* the root phase refuses a plan whose producer metadata says the plan was made by uid 0 (7.3).
- If a future distribution requirement forces a setuid or polkit integration (a GUI, or a non-terminal apply), the analysis in 4.3 and 4.4 applies unchanged: that path needs its own threat model and its own D6e-equivalent decision. It is out of scope for v1, which is a terminal tool.
- If plan transport over an inherited fd proves unreliable under the user's `sudo` configuration (`sudo` with a closed stdin, or `requiretty`), there is no second transport to fall back to: 7.2 admits an inherited descriptor as the only form, so the fix belongs in the handoff (for example keeping the descriptor open across the privilege transition), never in a plan file that root would have to open by path.

## 5. Trusted root handoff

### 5.1 Principle

The privileged phase may re-read `/etc/apt` (it must, to detect races and to verify digests) but may not re-derive decisions. Every decision — which mirror, which files, which bytes, which baseline, whether verification passed — is made in the unprivileged phase and travels as data. The privileged phase validates that data against a closed schema and against the current on-disk state, and refuses anything it cannot confirm. It has no fallback and no self-repair: a mismatch aborts with nothing written.

Why this and not "send the selection and let root recompute": recomputation would require the privileged phase to host the inventory parser and the rewrite engine, which (a) puts untrusted-input parsing in the root path, contradicting `SPEC.md` Privilege boundary bullet 3, and (b) means the bytes written are not the bytes the user confirmed in the diff (`SPEC.md` TUI, step 2, "old URI -> new URI"). Wrong on both counts.

### 5.2 Shape

```rust
// Written by the unprivileged phase, read and re-validated by the privileged phase.
struct WritePlan {
    format: u16,              // exact match required; a mismatch is a hard abort
    producer_uid: u32,        // refused when 0: the network phase must not run as root
    codename: String,         // == the codename read from os-release by the same run
    selected_uri: String,     // mirrors::validate_url shape + baseline proof below
    proof: WriteProof,        // section 3.9/3.10, minted by verify, not constructible elsewhere
    files: Vec<PlannedWrite>,
}

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
    content: Vec<u8>,          // the exact new bytes, byte-for-byte as shown in the diff
    create_new: bool,          // true only for the Pop!_OS security split file
}

enum Target {
    SourcesList,                        // /etc/apt/sources.list
    SourcesListD(FileName),             // /etc/apt/sources.list.d/<name>
}

struct FileName(String);               // validated: ^[A-Za-z0-9._-]{1,64}$ and .list|.sources
```

No path strings cross the boundary. `Target` is closed, and `FileName` cannot contain `/`, `.` alone, or `..`, so containment under `/etc/apt` holds by construction; a reviewer does not have to check a prefix comparison. `alternatives` need no separate field: they are `# alternative:` comment lines inside `content`, already rendered.

### 5.3 Encoding and transport

Encoding: a length-prefixed container — magic `APTSCPLAN`, format `u16`, then tagged records (`u16 tag`, `u32 length`, bytes). Reasons: `content` is arbitrary bytes and needs no escaping; a text format would need an escaping rule, and an escaping bug in the privileged parser is exactly the class of bug this handoff must not have. The parser is pure, lives outside the privileged code path, is unit-tested with hostile inputs (unknown tag, duplicate tag, oversize field, truncated record, non-UTF-8 where UTF-8 is required), and rejects everything it does not explicitly accept.

Transport: the plan arrives on an inherited file descriptor (stdin), and no plan-path argument exists at all (7.2). A path argument is A11: the privileged phase would read a file from a directory the invoking user can write. With stdin, the user's `sudo` command line carries no writable path:

```
apt-scout --apply --plan-out - | sudo apt-scout --plan-in -
```

The plan is at most a few hundred KiB (bounded by the 512 KiB `InRelease` cap and by the number of source files); the reader enforces a 1 MiB limit and aborts beyond it. `--plan-in` accepts `-` and nothing else, so there is no plan-file form to guard: the privileged phase never opens a path to obtain a plan, and no directory has to be trusted for one.

### 5.4 Who validates what

| Field | Unprivileged phase | Privileged phase |
| --- | --- | --- |
| `format` | writes the current version | equal to its own; else abort |
| `producer_uid` | writes its own effective uid | must not be 0; a plan from a root-run producer is refused (7.3) |
| `codename` | from os-release, used to build URLs | equals the codename read from `/etc/os-release` in this run; else abort |
| `selected_uri` | `mirrors::validate_url`, came from the ranked candidate set | `mirrors::validate_url` again (never trust a shape), and it must match the `content` it appears in |
| `proof` | minted by the verification pass: signed-region date + `Verified` | present, internally consistent, and its date matches the baseline logic that ranked it; a plan without a proof is refused, not downgraded |
| `target` | derived from the inventory | enum re-checked; the name re-validated; the resulting path `symlink_metadata`-checked as a regular file, uid 0, and the *directory* `/etc/apt/sources.list.d` checked for root ownership and no group/other write |
| `original_digest`/`mode`/`uid`/`gid` | reads them at plan time | re-reads at open time on the opened handle and compares; any difference aborts before any write |
| `content` | produced by `sources::rewrite_source` (pure) | used verbatim; no re-render |
| `create_new` | true only for `system-security.sources` | open with `create_new`; `EEXIST` aborts the whole apply (7.8) |
| backup root | not touched | created 0700 with post-creation owner/mode verification; a pre-existing directory that fails those checks aborts (7.6) |
| ordering | none | validate every file, then back up every file, then write every file; a backup failure aborts before the first write |

### 5.5 Why the boundary matters

If the privileged phase re-reads anything and acts on it without comparing to the plan, three failures become reachable that are otherwise closed: a local user's last-moment file substitution becomes a root write (B2); a mirror's forged date becomes a written URI (3.9); and the bytes written stop matching what the user confirmed. The handoff shape above closes all three with one mechanism — a closed schema plus digest comparison against current disk state — and it makes the security property reviewable without reading the write implementation.

## 6. Security acceptance tests that must exist before D9 merges

Checklist for a human reviewer. Every test runs in a disposable Ubuntu container with an isolated `/etc/apt` fixture and `--network none` where the case permits; none may touch a host path. "Fails when" describes the assertion, so a reviewer can check the test is real.

- **S1 — Non-root refusal.** `--apply` as uid 1000: exit 3, no file opened, message names the exact `sudo` command. *Fails when* the process writes anything, or exits 0/1, or the message omits the command.
- **S2 — Root phase without a plan.** `--plan-in -` with empty stdin, truncated container, bad magic, or a future `format`: exit 2, no file opened. *Fails when* any partial parse reaches a target.
- **S3 — Plan schema hostility.** Fixtures: unknown tag, duplicate tag, 5 MiB field, `..` inside a `FileName`, `system-security.sources/../sources.list`, absolute name, NUL byte, non-UTF-8 codename. Every one is refused before any open. *Fails when* one is accepted or produces a path outside `/etc/apt`.
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
- **S15 — The D6f P1 regressions gate the write, not only the display.** `noble-updates-InRelease.prefixed` (forged `Date:` before the armour): read-only run ranks it under the current tolerance, write run refuses to select it and refuses a plan naming it. Plus a plan claiming `Verified` for an unverified mirror is refused. *Fails when* either path accepts it.
- **S16 — Disposable-container rule, enforced by the harness.** `scripts/docker-test.sh` refuses to run the privileged cases unless it is inside a container it created and `/etc/apt` is a fixture path (for example `APT_SCOUT_ETC=/etc/apt` inside the container, never a bind mount of the host's). *Fails when* a privileged case can be pointed at a host path.

## 7. Decision record

Ruled 2026-11-12 by the user, relayed by the parent orchestrator. These are rulings, not proposals: the body above and `SPEC.md` are read as decided, and the remaining work is implementation (D6f, D9–D12). Each entry names the section that carries the ruling into behaviour and the tests that check it.

- **7.1 Self-reexec — Option A.** No self-reexec, no setuid bit, no internal `sudo`. A non-root `--apply` writes nothing, exits 3, and prints the exact two-invocation command. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 4.2 and 4.5; `SPEC.md` Privilege boundary bullet 4, Apply bullet 1, CLI; S1.
- **7.2 Plan transport — inherited fd only.** The plan crosses on stdin and nowhere else; no plan-path argument exists at all. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.3; `SPEC.md` CLI; S2.
- **7.3 Plan author.** The root phase refuses a plan produced by uid 0, so running the whole command under `sudo` cannot put the network phase in the root path. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 4.6, 5.2 and 5.4; `SPEC.md` CLI; S13.
- **7.4 D6f P1 #1.** For a write, `Date` is parsed only inside the cleartext-signed region; any non-whitespace before `-----BEGIN PGP SIGNED MESSAGE-----` is rejected. Read-only ranking keeps its current tolerance. DECIDED 2026-11-12 by the user (via the parent orchestrator). Implemented by D6f in `fix/d6f-verified-region`; this document does not change code. Carried by 3.9; `SPEC.md` Pipeline step 8; S15.
- **7.5 D6f P1 #2.** One proof value — signed-region date plus verification result, private constructor — is carried through `fresh::Probed` and the baseline; `fallback_baseline` accepts only verified candidate dates; the write fails closed when nothing verifies. DECIDED 2026-11-12 by the user (via the parent orchestrator). Implemented by D6f in `fix/d6f-verified-region`; this document does not change code. Carried by 3.10 and 5.2; `SPEC.md` Freshness trust boundary (Appendix A item 4); S15.
- **7.6 Backup root.** Keep `/var/backups/apt-scout`. A pre-existing path that is not root-owned, is not mode 0700, or is a symlink is a hard abort with nothing written; the `create_dir_all` self-heal no longer covers it. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by B5 and 5.4; `SPEC.md` Errors and self-healing; S9.
- **7.7 `--restore`.** Restore uses the same plan and the same privileged phase as `--apply`: one privileged entry point, and restore is another kind of plan. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.1 and 5.4; `SPEC.md` Apply and CLI; S4, S10.
- **7.8 Pop!_OS new file.** `/etc/apt/sources.list.d/system-security.sources` is created with exclusive create; `EEXIST` aborts the whole apply, so a file the run did not plan is never overwritten. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by B8 and 5.4; `SPEC.md` Apply; S8.
- **7.9 Plan encoding.** Length-prefixed tagged container: no escaping, a pure parser, hostile-input unit tests. DECIDED 2026-11-12 by the user (via the parent orchestrator). Carried by 5.3; `SPEC.md` Apply; S2, S3.

Appendix A records what each ruling changed in `SPEC.md`, and the owner of every delta that is not yet applied.

---

## Appendix A — SPEC delta ledger

State of every delta the rulings in section 7 require. APPLIED names the `SPEC.md` section that now carries the requirement; PENDING names the owner. No item is left unowned.

1. **APPLIED — `SPEC.md` "Privilege boundary and threat model", bullet 4.** Replaced "Do not use privileged self-reexec of the current executable until D6e approves a concrete trust mechanism." with "The program never re-execs itself privileged, never invokes `sudo`, and is never installed setuid. Privilege is obtained only by the user running the program as root. The root invocation is a separate mode that reads a validated plan and performs file operations only; it makes no network request, runs no external process, resolves no command through `PATH`, and reads no environment variable."
2. **APPLIED — `SPEC.md` "Apply", bullet 1.** Replaced "Do not implement self-reexec through `sudo` or another privileged helper until the privilege-boundary design passes D6e security audit." with "Discovery, verification, rewriting, and confirmation produce a plan that is the root phase's only input. The plan is a closed schema: it names files through a fixed enumeration, carries the exact bytes to write, carries the verification result for the selected mirror and the freshness baseline, and carries the digest, mode, and owner of every file it was computed from. The root phase aborts, before its first write, on any missing field, any mismatch with the file on disk, and any plan it did not fully validate."
3. **APPLIED — `SPEC.md` "Pipeline", step 8.** After "`Date` is the signing time, not the sync time." added "The `Date` used for a write comes from inside the cleartext-signed region: bytes before `-----BEGIN PGP SIGNED MESSAGE-----` must be ASCII whitespace only, and the header is read between the armour headers and the signature. A `Date` outside that region is used for read-only ranking only."
4. **PENDING (D6f) — `SPEC.md` "Privilege boundary and threat model", "Freshness trust boundary".** Owner: D6f in `fix/d6f-verified-region`, together with 7.5. Text to add after "fallback baselines are acceptable only when computed from verified candidate dates.": "The verification result and the signed-region date are one value produced by the verification pass; a status that can be constructed without that pass must not exist, and a plan without it is refused rather than downgraded. A fallback baseline may include only candidates whose `-updates` `InRelease` was verified in the same run."
5. **APPLIED — `SPEC.md` "CLI", table rows `--apply`/`--restore` and the behaviour bullets; "Apply", the `--restore` bullet.** Applied narrower than the draft below, because 7.2 admits no plan-path form: `--plan-out -` prints the plan on stdout, `--plan-in -` reads it from stdin, and any value other than `-` is refused. `--apply` without root writes nothing, exits 3 and prints `apt-scout --apply --plan-out - | sudo apt-scout --plan-in -`; the privileged mode is reached only through that second invocation and refuses a plan produced by uid 0. `--restore` builds its plan from the newest backup and enters the same privileged mode, so one entry point serves both. Draft: "Add the plan flags to the table (`--plan-out <path|->`, `--plan-in <path|->`) and state that `--apply` without root exits 3 and prints the exact command, and that the privileged mode is reached only through a second, explicit invocation."
6. **APPLIED — `SPEC.md` "Errors and self-healing", the self-heal bullet.** "Missing own directories (`~/.cache/apt-scout`, `/var/backups/apt-scout`): create with `create_dir_all`." was replaced with "Missing own cache directory: create with `create_dir_all`. `/var/backups/apt-scout` is created by the root phase and verified after creation to be a directory, owned by root, mode 0700, and not a symlink; a pre-existing path that fails any of these is a hard abort with nothing written."
7. **APPLIED — `SPEC.md` "Errors and self-healing", the Docker bullet.** The drafted section name "Testing" is wrong: the anchor sentence ("Docker integration tests for source discovery/apply/restore must use disposable Ubuntu containers and isolated `/etc/apt` fixtures.") lives in Errors and self-healing and is present verbatim there, so the text was appended to it: "Privileged-write tests cover: non-root refusal, plan schema hostility including path traversal and oversize fields, symlinked targets and a symlinked `sources.list.d`, substitution between validation and write, a read-only `/etc/apt` mount, backup failure aborting before any write, byte-exact restore of content/mode/owner, rejection of paths outside `/etc/apt`, hostile `PATH`/`HOME`/`TMPDIR`, and a run that succeeds with `gpgv` removed, proving the privileged phase does not re-verify."
8. **PENDING (D9) — `GLOSSARY.md`.** Owner: D9, the milestone that introduces the privileged phase; this document does not edit `GLOSSARY.md`. Three terms, vocabulary only: **Plan** — the validated description of a pending apt change, carrying the bytes to write and the proof they came from verified data; it is the only input to the privileged phase. **Privileged phase** — the part of a run that holds root; it performs file operations and nothing else. **Write proof** — the value that ties an `InRelease` date to a successful signature verification.

---

## Appendix B — accepted risks and items owned elsewhere

Recorded so nothing here is dropped silently; none of these is an open decision.

- The throughput sample is attacker-controlled (`src/probe.rs:153–156`) and can decide the ranking. Accepted: it cannot authorize package installation, and `SPEC.md` Pipeline step 9 already calls throughput relative. Owner: D9, with no SPEC delta.
- Root printing an unescaped mirror URL or error string into a terminal (B7) is only a hazard once a privileged phase prints anything. The mitigation belongs to D13, not D9.
- `reader.join()` is unbounded after the timeout kill in the verifier (`src/verify.rs:212–232`), and the oversized-body case reports `Unavailable` rather than `Invalid`. Both stay in D6f's P2 list; neither changes what a write may trust.
- `--list` is accepted and ignored (`src/main.rs:47`); that is a CLI conformance gap for D11, not a security issue.
